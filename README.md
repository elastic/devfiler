devfiler
=========

devfiler reimplements the whole collection, data storage, symbolization, and UI portion of
[OTel eBPF Profiler] in a desktop application. This essentially allows developers to start
using the profiling agent in a matter of seconds without having to spin up a whole Elastic 
deployment first.

[OTel eBPF Profiler]: https://github.com/open-telemetry/opentelemetry-ebpf-profiler/

devfiler currently supports running on macOS and Linux. Note that this doesn't mean that this
application can profile macOS applications: the [OTel eBPF Profiler] still needs to run on a Linux
machine, but the UI can be used on macOS.

> [!NOTE]
> 
> This is currently **not** a supported product. It started out as [@athre0z]'s personal
> project and was later transferred to the Elastic GitHub account because some people in
> the team liked the idea of having it to speed up some development workflows and
> prototyping. We're now releasing it under Apache-2.0 to help with OTLP Profiling
> development.

[@athre0z]: https://github.com/athre0z

<img width="1804" alt="screenshot1" src="https://github.com/elastic/devfiler/blob/e8d68d9176f39eea8b05293059a2baecff02aaee/assets/screenshot1.png">

<img width="1804" alt="screenshot2" src="https://github.com/elastic/devfiler/blob/e8d68d9176f39eea8b05293059a2baecff02aaee/assets/screenshot2.png">

## Build

### Nix

The primary build system is currently the [Nix] package manager. Once Nix is 
installed on the system, devfiler can be built with the following command:

```
nix --experimental-features 'flakes nix-command' build '.?submodules=1#'
```

The executable is placed in the Nix store and a symlink is created in the root of this directory.
You can then run devfiler using:

```
result/bin/devfiler
```

Alternatively you can simply ask Nix to both build and run it for you:

```
nix --experimental-features 'flakes nix-command' run '.?submodules=1#'
```

[Nix]: https://nixos.org/download

The need to always pass the `--experimental-features` argument can be circumvented by putting

```
experimental-features = nix-command flakes
```

into `~/.config/nix/nix.conf`.

### Cargo

Alternatively it's also possible to build devfiler with just plain cargo. This currently doesn't 
allow generating a proper application bundle for macOS, but it's perfectly sufficient for 
development and local use. Cargo is typically best installed via [rustup], but using `cargo` and 
`rustc` from your distribution repositories might work as well if it is recent enough.

[rustup]: https://rustup.rs/

Additionally, make sure that `g++` (or `clang`), `libclang` and `protoc` are available on 
the system. The following should do the job for Debian and Ubuntu. The packages should also 
be available in the repositories of other distributions and also from MacPorts/Brew, but
names may vary.

```
sudo apt install g++ libclang-dev protobuf-compiler libprotobuf-dev cmake
```

devfiler can then be built using:

```
# Update submodules only after cloning the repository or when the submodules change.
git submodule update --init --recursive

cargo build --release
```

The executable is placed in `target/release/devfiler`.

## Adding traces

devfiler is listening for profiling agent connections on `0.0.0.0:11000`. To ingest traces,
use a recent version of the OTel eBPF profiler and then run it like this:

```
sudo ./ebpf-profiler -collection-agent=127.0.0.1:11000 -disable-tls
```

### Profiling on remote hosts

A common use-case is to ssh into and run the profiling agent on a remote machine. The easiest
way to set up the connection in this case is with a [ssh reverse tunnel]. Simply run devfiler 
locally and then connect to your remote machine like this:

```
ssh -R11000:localhost:11000 someuser@somehost
```

This will cause sshd to listen on port `11000` on the remote machine, forwarding all connections
to port `11000` on the local machine. When you then run the profiling agent on the remote and point
it to `127.0.0.1:11000`, the connection will be forwarded to your local devfiler.

[ssh reverse tunnel]: https://unix.stackexchange.com/questions/46235/how-does-reverse-ssh-tunneling-work

## Developer mode

Some of the more internal tabs that are only relevant to developers are hidden by default. You can
unveil them with a double click on the "devfiler" text in the top left.

## Releases

<details>
<summary>Creating release artifacts locally</summary>

Update `version` in `Cargo.toml` for the package to the appropriate release version number

```
# On a linux machine, architecture doesn't matter as long as qemu binfmt is installed:
nix bundle --system aarch64-linux --inputs-from . --bundler 'github:ralismark/nix-appimage' '.?submodules=1#appImageWrapper' -L
nix bundle --system x86_64-linux  --inputs-from . --bundler 'github:ralismark/nix-appimage' '.?submodules=1#appImageWrapper' -L
# Resulting appimages are symlinked into CWD.

# On a ARM64 mac w/ Rosetta installed:
nix build -L '.?submodules=1#packages.aarch64-darwin.macAppZip' -j20
cp result/devfiler.zip devfiler-apple-silicon-mac.zip
nix build -L '.?submodules=1#packages.x86_64-darwin.macAppZip' -j20
cp result/devfiler.zip devfiler-intel-mac.zip
```

</details>

> [!NOTE]
>
> Binary releases are covered by multiple licenses (stemming from compiling and
> linking third-party library dependencies) and the user is responsible for reviewing
> these licenses and ensuring that license terms (e.g. redistribution and copyright
> attribution) are met.
>
> Elastic does not provide devfiler binary releases.
