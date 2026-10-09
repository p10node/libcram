# Native SDK engineering packages

`python3 scripts/package_sdk.py` builds the native C ABI with `--locked` for the
Rust host target, then creates a relocatable C/C++ SDK tarball in `target/dist/`.
It refuses a dirty checkout and existing output archives; it never publishes or
installs into system directories. Run from a committed, otherwise quiet checkout.
No cross-compilation, downloaded prebuilt libraries or substituted fixtures are
used for the native library. Python wheels and Go source builds retain their
existing independent binding gates in `docs/BINDINGS.md`.

Each package contains shared/static libraries, generated C declarations, both
C++ PMR headers, a relocatable `libcram.pc`, licenses, README and acceptance docs.
`MANIFEST.json` records the source commit, Rust compiler, target and SHA-256 of
every packaged file except the manifest itself. The filename contains the source
commit to distinguish engineering iterations without inventing a new release.
A package is **not production acceptance**: the bundled hardware/performance
reports remain authoritative. There is no automatic registry or GitHub release.

Example after extraction into an owned prefix:

```sh
export PKG_CONFIG_PATH="$prefix/lib/pkgconfig"
cc consumer.c $(pkg-config --cflags --libs libcram) \
  -Wl,-rpath,"$prefix/lib" -o consumer
./consumer
```

Static linking additionally needs the native system libraries reported by Rust
for the selected target; the pkg-config file above describes shared linking.
`cram_unregister_pressure_callback_and_wait()` provides a quiescent C userdata
release boundary. Ordinary callback replacement/unregister remains nonblocking;
serialize registrations and never hold a callback-needed lock while waiting.

Before distributing an engineering archive, run the composed host/Linux,
header, native/Python/Torch and focused runtime/adapter gates, then extract the
actual tarball into an owned temporary prefix, independently verify its manifest,
and compile/run both C and C++ consumers against **that extracted library**.
Long-duration soak, actual CRAM capacity benefits and active telemetry CPU
acceptance cannot be replaced by this packaging smoke check.
