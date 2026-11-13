# Av1an Documentation

Av1an is a video encoding library and command-line tool built for speed, ease of use, and extensibility.
It splits a video into scenes, encodes them in parallel, and stitches the result back together.
Optionally target a level of quality, analyze video quality, and get feedback through a terminal UI.

The command-line tool is **California Condor** (Cargo package `california-condor`,
binary `av1an`) which is built on top of the Rust library, **Andean Condor**.

## Where to start

| Section | What it covers |
| ------- | -------------- |
| [Guide](guide.md) | The end-to-end workflow and how the pieces fit together |
| [Commands](commands/condor.md) | Every `av1an` subcommand and its flags |
| [Configuration](configuration.md) | The `condor.json` file: global flags and validation |
| [Configuration Reference](configuration/index.md) | The full `condor.json` schema, field by field |
| [Types](types.md) | Decoders, encoders, filters, and other complex types |
| [Encoders](encoders/aomenc.md) | Per-encoder tuning notes |

## Install

As of 2026-11-13, `california-condor` is unpublished on crates.io and the checked
upstream releases do not yet provide Condor ZIPs. With the
[build prerequisites](compiling.md) installed, run from the root of a `main`
checkout containing this workspace:

```bash
cargo install --locked --path california-condor --bin av1an
```

**Windows:** this copies only `av1an.exe`, not its required DLLs. Follow the
[Windows runtime/build instructions](installation.md#installation)
instead of using this command alone.

See [Installation](installation.md) for availability and runtime dependencies,
[Compiling](compiling.md) to build without installing, and [Docker](docker.md) for
upstream image policy and local Condor builds.

## Library

`andean-condor` is also unpublished on crates.io as of 2026-11-13. Add a local path
dependency to your project's `Cargo.toml`, replacing the example path with the
checked-out library's path relative to your project:

```toml
[dependencies]
andean-condor = { path = "../Av1an/andean-condor" }
```

See the [library example](../../andean-condor/examples/simple.rs).
