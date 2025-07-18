{
  description = "devfiler: universal profiling as a desktop app";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
    crane.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { crane, flake-utils, nixpkgs, ... }:
    flake-utils.lib.eachSystem [
      "aarch64-linux"
      "x86_64-linux"
      "aarch64-darwin"
      "x86_64-darwin"
    ]
      (system:
        let
          pkgs = import nixpkgs { inherit system; };
          llvm = pkgs.llvmPackages_16;
          stdenv = llvm.stdenv;
          lib = pkgs.lib;
          isLinux = stdenv.isLinux;
          isDarwin = stdenv.isDarwin;
          craneLib = (crane.mkLib pkgs);

          # Filter source tree to avoid unnecessary rebuilds.
          includedSuffixes = [
            ".proto"
            "metrics.json"
            "errors.json"
            "icon.png"
            "add-data.md"
            "README.md"
          ];
          isBuildInput = p: lib.any (x: lib.hasSuffix x p) includedSuffixes;
          devfilerSources = lib.cleanSourceWith {
            src = lib.cleanSource (craneLib.path ./.);
            filter = (o: t: (craneLib.filterCargoSources o t) || (isBuildInput o));
          };
          assets = builtins.path {
            path = ./assets;
            name = "devfiler-assets";
          };

          # RocksDB library to be used.
          rocksdb = stdenv.mkDerivation rec {
            name = "rocksdb";
            version = "8.10.0"; # must match what the Rust bindings expect!
            src = pkgs.fetchFromGitHub {
              owner = "facebook";
              repo = "rocksdb";
              rev = "v${version}";
              hash = "sha256-KGsYDBc1fz/90YYNGwlZ0LUKXYsP1zyhP29TnRQwgjQ=";
            };
            nativeBuildInputs = with pkgs; [ cmake ninja ];
            propagatedBuildInputs = with pkgs; [ zstd ];
            env.NIX_CFLAGS_COMPILE = lib.optionalString stdenv.cc.isClang "-faligned-allocation";
            cmakeFlags = [
              "-DPORTABLE=1" # suppress -march=native
              "-DWITH_ZSTD=ON"
              #"-DWITH_JEMALLOC=ON"
              "-DWITH_TOOLS=OFF"
              "-DWITH_CORE_TOOLS=OFF"
              "-DWITH_BENCHMARK_TOOLS=OFF"
              "-DWITH_TESTS=OFF"
              "-DWITH_JNI=OFF"
              "-DWITH_GFLAGS=OFF"
              "-DROCKSDB_BUILD_SHARED=OFF"
              "-DFAIL_ON_WARNINGS=OFF"
            ];
            dontFixup = true;
          };

          # On Linux egui dynamically links against X11 and OpenGL. The libraries
          # listed below are injected into the RPATH to ensure that our executable
          # finds them at runtime.
          linuxDynamicLibs = lib.makeLibraryPath (with pkgs; with xorg; [
            libGL
            libX11
            libxkbcommon
            libXcursor
            libXrandr
            libXi
          ]);

          buildDevfiler =
            { profile ? "release"
            , extraFeatures ? [ "automagic-symbols" "allow-dev-mode" ]
            }: craneLib.buildPackage {
              inherit stdenv;
              strictDeps = true;
              src = devfilerSources;
              doCheck = false;
              dontStrip = true;
              dontPatchELF = true; # we do this ourselves
              meta.mainProgram = "devfiler";

              buildInputs = [
                rocksdb
              ] ++ lib.optional isLinux [
                pkgs.libcxx
                pkgs.openssl
                pkgs.gcc-unwrapped
              ] ++ lib.optional isDarwin [
                pkgs.libiconv
                pkgs.darwin.apple_sdk.frameworks.CoreServices
                pkgs.darwin.apple_sdk.frameworks.AppKit
              ];

              nativeBuildInputs = with pkgs; [ cmake protobuf copyDesktopItems ]
                ++ lib.optional isDarwin desktopToDarwinBundle
                ++ lib.optional isLinux pkg-config;

              desktopItems = pkgs.makeDesktopItem {
                name = "devfiler";
                exec = "devfiler";
                comment = "Elastic Universal Profiling desktop app";
                desktopName = "devfiler";
                icon = "devfiler";
              };

              cargoExtraArgs =
                let
                  # wgpu renderer is generally preferable because it uses Metal (macOS)
                  # or Vulkan (Linux). Unfortuantely it hard-freezes some people's Linux
                  # kernel when running on Intel drivers. Only use it on macOS for now.
                  renderer = if isDarwin then "render-wgpu" else "render-opengl";
                  features = [ renderer ] ++ extraFeatures;
                  merged = lib.concatStringsSep "," features;
                in
                "--no-default-features --features ${merged}";

              env = {
                # Use our custom build of RocksDB (instead of letting cargo build it).
                ROCKSDB_INCLUDE_DIR = "${rocksdb}/include";
                ROCKSDB_LIB_DIR = "${rocksdb}/lib";
                ROCKSDB_STATIC = "1";

                # libclang required by rocksdb-rs bindgen.
                LIBCLANG_PATH = llvm.libclang.lib + "/lib/";

                CARGO_PROFILE = profile;

                RUSTFLAGS = toString [
                  # Mold speeds up the build by a few seconds.
                  # It doesn't support macOS: only use it on Linux.
                  (lib.optional isLinux "-Clink-arg=--ld-path=${pkgs.mold-wrapped}/bin/mold")

                  # On Darwin, librocksdb-sys links C++ libraries in some weird
                  # way that doesn't work with `buildInputs`. Link it manually ...
                  (lib.optionals isDarwin [
                    "-L${pkgs.libcxx}/lib"
                    "-ldylib=c++"
                    "-ldylib=c++abi"
                  ])
                ];
              } // lib.optionalAttrs isLinux {
                PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
              };

              preInstall = ''
                install -Dm644 ${assets}/icon.png \
                  $out/share/icons/hicolor/512x512/apps/devfiler.png
              '';
              postInstall = lib.optionalString isLinux ''
                patchelf --shrink-rpath $out/bin/devfiler
                patchelf --add-rpath ${linuxDynamicLibs} $out/bin/devfiler
              '';

              # On macOS, ship the required C++ runtime libs as part of
              # the application bundle that we are building.
              postFixup = lib.optionalString isDarwin ''
                ppp=$out/Applications/devfiler.app/Contents/MacOS/
                if [[ -d $ppp ]]; then  # don't run in "deps" step
                  mv $out/bin/devfiler $ppp
                  cp ${pkgs.libcxx}/lib/libc++.1.0.dylib $ppp
                  cp ${pkgs.libcxx}/lib/libc++abi.1.dylib $ppp

                  # Make files writable
                  chmod +w $ppp/devfiler
                  chmod +w $ppp/libc++.1.0.dylib
                  chmod +w $ppp/libc++abi.1.dylib

                  # Fix the main executable
                  install_name_tool \
                    -change ${pkgs.libcxx}/lib/libc++.1.0.dylib \
                      @executable_path/libc++.1.0.dylib \
                    -change ${pkgs.libcxx}/lib/libc++abi.1.0.dylib \
                      @executable_path/libc++abi.1.dylib \
                    -change ${pkgs.libiconv}/lib/libiconv.dylib \
                      /usr/lib/libiconv.2.dylib \
                    $ppp/devfiler

                  # Fix libc++.1.0.dylib's dependencies
                  install_name_tool \
                    -change ${pkgs.libcxx}/lib/libc++abi.1.dylib \
                      @executable_path/libc++abi.1.dylib \
                    $ppp/libc++.1.0.dylib
                fi
              '';
            };

          devfilerCheckRustfmt = craneLib.cargoFmt {
            src = devfilerSources;
          };

          macSystemName = {
            "aarch64-darwin" = "apple-silicon";
            "x86_64-darwin" = "intel-mac";
          }.${system} or (throw "unsupported mac system: ${system}");

          macAppZip = pkgs.runCommand "devfiler-mac-app" {
            nativeBuildInputs = [ pkgs.zip ];
          } ''
            # Copy and change permissions. Without this the app extracted from
            # the zip will be read-only and require extra steps to move around.
            cp -rL ${buildDevfiler {}}/Applications/devfiler.app .
            chmod -R u+w .

            install -d $out
            zip -r $out/devfiler-${macSystemName}.app.zip devfiler.app
          '';

          # Build the contents of our AppImage package.
          #
          # 1) We need to strip the Nix specific `linuxDynamicLibs` library paths
          #    that contain X11 and OpenGL libraries. They won't work on regular
          #    distributions because the corresponding user-mode graphics drivers
          #    will be missing. We need to load the native distro libs for that.
          # 2) Nix's glibc is patched to ignore `/etc/ld.so.conf`. This is what
          #    allows it to co-exist on regular distros and makes sure that Nix
          #    executables don't accidentally load regular distro libs. However,
          #    in the case of our AppImage, that works against us: egui loads
          #    X11/wayland/OpenGL libraries dynamically and we need it to find
          #    the distro libraries. We achieve this with a wrapper that sets
          #    a custom LD_LIBRARY_PATH that **prefers** Nix libraries, but has
          #    the ability to fall back to distro lib dirs when needed. This
          #    combines the best of two worlds: we ship most libraries with
          #    us and ditch potential ABI issues for those and load distro libs
          #    for stuff that simply isn't portable (crucially: OpenGL).
          appImageLibDirs = [
            # Nix system paths
            "${pkgs.glibc}/lib"
            "${pkgs.stdenv.cc.libc.libgcc.libgcc}/lib"

            # Distro library paths
            "/usr/lib/${system}-gnu" # Debian, Ubuntu
            "/usr/lib" # Arch, Alpine
            "/usr/lib64" # Fedora
          ];
          appImageDevfiler = pkgs.runCommand "devfiler-stripped"
            {
              env.unstripped = buildDevfiler { };
              nativeBuildInputs = with pkgs; [ binutils patchelf ];
              meta.mainProgram = "devfiler";
            } ''
            cp -R $unstripped $out
            chmod -R +w $out
            strip $out/bin/devfiler
            patchelf --shrink-rpath $out/bin/devfiler
          '';
          appImageWrapper = pkgs.writeShellScriptBin "devfiler-appimage" ''
            export LD_LIBRARY_PATH=${lib.concatStringsSep ":" appImageLibDirs}
            ${lib.getExe appImageDevfiler} "$@"
          '';

          # Wrapped variant of devfiler that uses the Distro's libgl.
          devfilerDistroGL = pkgs.writeShellScriptBin "devfiler-distro-gl" ''
            export LD_LIBRARY_PATH=${lib.concatStringsSep ":" appImageLibDirs}
            ${lib.getExe (buildDevfiler {})} "$@"
          '';

          # Provides a basic development shell with all dependencies.
          devShell = pkgs.mkShell {
            packages = with pkgs; [ cargo ];
            inputsFrom = [ (buildDevfiler { profile = "dev"; }) ];
            LIBCLANG_PATH = llvm.libclang.lib + "/lib/";
            LD_LIBRARY_PATH = lib.optionalString isLinux linuxDynamicLibs;
          };
        in
        {
          formatter = pkgs.nixpkgs-fmt;
          devShells.default = devShell;
          packages = {
            inherit rocksdb;
            default = buildDevfiler { };
            release = buildDevfiler { };
            dev = buildDevfiler { profile = "dev"; };
            lto = buildDevfiler { profile = "release-lto"; };
          } // lib.optionalAttrs isDarwin {
            inherit macAppZip;
          } // lib.optionalAttrs isLinux {
            inherit appImageWrapper devfilerDistroGL;
          };
          checks.rustfmt = devfilerCheckRustfmt;
        }
      );
}

