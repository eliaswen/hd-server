# Install

## Prerequisites

- A Rust toolchain (stable, edition 2024) with `cargo`.
- A copy of Hay Day **1.43.150 (build 1598)** for Android that you are entitled
  to use, x86 or arm64-v8a.
- [hd-dumper](https://github.com/eliaswen/hd-dumper), to generate the runtime
  data tables.
- [hd-patcher](https://github.com/eliaswen/hd-patcher), to repackage the client
  so it connects to this server.
- A device or emulator that can reach the machine running the server.

## 1. Build or install

```sh
cargo build --release
```

The binary is `target/release/hd-server`. Run `hd-server --help` for every
option.

Each [release](https://github.com/eliaswen/hd-server/releases) also has
archives for Linux and macOS, and deb and rpm packages are served from a signed
apt and dnf repository; [eliaswen.github.io/hd-server](https://eliaswen.github.io/hd-server/)
has the setup commands. The packages install:

- the `hd-server.service` systemd unit, which is **not enabled or started**
  until you set it up;
- the configuration as `/etc/hd-server/config.toml`, kept across upgrades;
- a state directory, `/var/lib/hd-server`, that the service runs from, so
  `resources/`, `data/` and `dumps/` live there.

With the packages, the steps below become: generate resources into
`/var/lib/hd-server/resources`, edit `/etc/hd-server/config.toml`, run
`sudo systemctl enable --now hd-server`, and read the public key with
`journalctl -u hd-server`.

## 2. Generate `resources/`

Run hd-dumper against your copy of the game and put its output in
`resources/` (or set `resources_dir` to another path, relative to the working
directory). The server reads and checks every table when it starts, and
**fails to start if the directory or any table is missing or unreadable**,
naming the files to regenerate.

The directory is not part of this repository and is ignored by git. Refresh
older reduced runtime tables with hd-dumper to get the full columns used by
expansion, pet feeding, decoration pricing, and service upgrade replay.

## 3. Configure

```sh
cp config.toml.example config.toml
```

The example is annotated. At minimum, review:

- `http_listen` and `game_listen`: the defaults bind to loopback. Use
  `0.0.0.0:8080` and `0.0.0.0:9339` (or a specific interface) for a device on
  another host.
- `game_host`: the address the client is told to open the game connection to.
  See [network setup](#5-network-setup).

[Usage](usage.md) describes every setting.

## 4. Get the server key and patch the client

Start the server once:

```sh
hd-server --config config.toml
```

On first start it creates a private 32-byte X25519 secret (`--key-file`, default
`dumps/server-key.bin`, owner-only permissions) and logs its **public key**.
Pass that key to hd-patcher with `--server-public-key` to build a client that
trusts this server.

Keep the key file. If it changes, every client has to be patched again.

## 5. Network setup

The client opens two connections:

1. An HTTP request for `/env_settings`. This file contains the `serverHost` and `serverPort` for the game connection. The
   server listens on `http_listen` (default `0.0.0.0:8080`). By default, the game client connects to `http://gamea.haydaygame.com:8080`.
   With [hd-patcher](https://github.com/eliaswen/hd-patcher), you can configure the client to use the server's HTTP endpoint directly.
   Otherwise, you can use DNS, HTTP proxy or a hosts file to redirect `gamea.haydaygame.com` to the server's IP address and port.
2. A TCP connection to the `serverHost` returned by bootstrap, which is
   `game_host`, on port 9339.

By default, the client refuses private IPv4 ranges (such as `10.0.0.0/8` and
`192.168.0.0/16`) for the game connection. This can changed with [hd-patcher](https://github.com/eliaswen/hd-patcher)'s `--allow-local-ranges` option.
Otherwise, you'll need to use a public address, or one from the benchmarking range `198.18.0.0/15` that the device can reach. Assigning
that address to an interface on the host, or routing it, is up to you.

## 6. Play

Install the patched client and start it. A new account is created on first
login and gets a starter farm. See [usage](usage.md#accounts-and-farms) for
what is saved and where.
