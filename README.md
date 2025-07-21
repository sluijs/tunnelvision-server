# Tunnelvision Server
A webserver for Tunnelvision with WebSocket support written in Rust. 

This server is used within the `tunnelvision` Python package to communicate between the Python runtime and the front-end through WebSockets, while also serving the front-end over HTTP.

## Installation
```bash
# Install Rust and Cargo
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Create a release build
cargo build --release

# Copy the executable to the tunnelvision package
cp ./target/release/tunnelvision-server ../tunnelvision/tunnelvision/bin
```

## VS Code Remote
To use tunnelvision through VS Code remote, we need forward an arbitrary available port to the tunnelvision-server. Once you have forwarded a port from the `ports` pane within VS Code, make sure to add it to your configuration file for tunnelvision:

```yaml
# ~/.cache/tunnelvision/default_config.yaml
port: 1337
```

## Debug
The `stdout` and `stderr` are logged in `~/.cache/tunnelvision/server.out`.
