# Developing and Contributing

## Opening a Pull Request

When committing changes, it is recommended to run several commands to make sure the code compiles and adheres to the configured formatting and linting rules.

### `cargo check`

This is the most basic check to make sure the code compiles.

### `cargo clippy`

In addition to making sure the code compiles, [Clippy](https://github.com/rust-lang/rust-clippy) will also provide linting and code analysis. You can append `--fix` to automatically fix trivial issues.

### `cargo fmt`

This command will automatically format the code and uses the rustfmt.toml file for configuration.

### `cargo build`

Build the project. By default, this will build for the `debug` config. Run `cargo build --release` for the `release` config.

### `cargo test`

Execute all unit and integration tests across the project to ensure that the code is working as expected.

## Configuring Visual Studio Code

If you are using [Visual Studio Code](https://code.visualstudio.com/) for development, there are a few things you may want to configure. The most helpful of which is to use the [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer) extension. If you want syntax highlighting and formatting for TOML files such as Cargo.toml, you can install the [Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml) extension. For developing in a container, install the [Dev Containers](https://marketplace.visualstudio.com/items?itemName=ms-vscode-remote.remote-containers) extension.

### rust-analyzer with Clippy

The `cargo check` command is great for making sure that the code compiles but it does not provide linting or code analysis for Visual Studio Code. The `rust-analyzer` extension provides these additional features with the use of `cargo clippy`. Clippy will also account for additional formatting rules.

1. Navigate to *File > Preferences > Settings > Extensions > rust-analyzer > check: Command*
2. Switch from **User** to **Workspace**
3. Change `rust-analyzer.check.command` to `clippy`
