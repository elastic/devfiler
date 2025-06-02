### Adding traces

devfiler is listening for profiling agent connections on `0.0.0.0:11000`. To ingest traces build
`opentelemetry-ebpf-profiler` from source from [this repository].

[this repository]: https://github.com/open-telemetry/opentelemetry-ebpf-profiler

Remember the path that the `ebpf-profiler` was built in, then run it like so:

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

### Adding symbols for native executables

Symbols for native executables can be added by navigating to the "Executables" tab in devfiler,
then simply dragging and dropping the executable anywhere within the window. A progress indicator
shows up during ingestion.
