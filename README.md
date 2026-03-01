# Optra

A GPU-accelerated image viewer for high dynamic range work, built with Rust, `wgpu` and `egui`.

Optra keeps EXR and Radiance HDR images as linear float data all the way to the
GPU, so exposure and gamma are adjusted live in the fragment shader rather than
by re-decoding the file. It is built for stepping through rendered frame
sequences — locally or on a remote machine over SSH.

## Features

- **Hardware-accelerated display.** Images upload straight to a GPU texture; pan
  and zoom are a transform in the shader, not a CPU resample.
- **True HDR.** `.exr` and `.hdr` load as `Rgba32Float` and keep their full
  range, with exposure (EV) and gamma controls. Highlights above 1.0 survive, so
  they can be pulled back rather than clipped to white.
- **Frame sequences.** Files whose names differ only by a number
  (`frame_0007.exr`, `render.0008.exr`) are recognised as one sequence. Arrow
  keys scrub through them at a configurable rate, and the view — zoom, pan,
  exposure, gamma — holds steady across frames instead of resetting.
- **Background preloading.** Neighbouring frames are decoded on worker threads
  so scrubbing does not stall on I/O.
- **Remote browsing.** Connect to any machine you can `ssh` into and browse its
  filesystem as if it were local, over SFTP.
- **Theming.** All four Catppuccin flavours, including the transparency
  checkerboard.

## Platform support

| Platform | Status |
|---|---|
| macOS | Supported. Ships as an `.app` bundle with Finder integration. |
| Linux | Supported — built and tested in CI. |
| Windows | **Not yet supported. [Help wanted](#help-wanted-windows-port).** |

## Installing

### macOS

```bash
./scripts/bundle.sh
cp -r dist/Optra.app /Applications/
```

This builds a release binary, assembles the `.app`, generates a rounded macOS-style icon from
`logo.png`, and ad-hoc signs the bundle. Optra then registers as a handler for
image files: it claims `Owner` rank for OpenEXR and Radiance HDR, and
`Alternate` for common formats like PNG and JPEG, so it appears under
"Open With" without taking over from Preview.

The signature is ad-hoc, not a Developer ID, so the bundle is not notarised —
fine locally, but Gatekeeper will object if you pass it to someone else.

### Linux, or from source

```bash
cargo run --release -- path/to/image.exr
```

Debian/Ubuntu build dependencies:

```bash
sudo apt-get install libxkbcommon-dev libwayland-dev libxcb1-dev \
  libxrandr-dev libxi-dev libxcursor-dev libgl1-mesa-dev pkg-config
```

## Using it

| Action | Control |
|---|---|
| Zoom | Scroll (zooms toward the cursor) |
| Pan | Click and drag |
| Previous / next frame | Left / Right arrow (hold to scrub) |
| Open a file | Click it in the tree, drag it onto the window, or pass it on the command line |
| Re-root the tree | Right-click a folder → **Set as root**, or ⬆ for the parent |
| Settings | The ⚙ button above the tree |

Exposure and gamma appear in the side panel and are enabled only for HDR images;
8-bit images are already display-encoded and are passed through untouched.

## Remote viewing

Click **🌐 Remote**, enter a host, user and port, and Optra connects over SFTP.
The file tree, sequence detection, and preloading all work the same as local.
**⏏ Disconnect** returns to the local filesystem.

Two requirements, both matching what `ssh` itself expects:

1. **The host must be in `~/.ssh/known_hosts`.** Optra verifies host keys and
   refuses unknown ones rather than trusting on first use. Run
   `ssh user@host` once to record the key.
2. **Your key must be loaded in the SSH agent.** Optra authenticates through the
   agent only and never reads key files or handles passwords. If `ssh` works but
   Optra does not, run `ssh-add`.

Images are decoded locally, so the whole file crosses the network. That is fine
on a LAN; a 130 MB EXR over a VPN will not be.

## Help wanted: Windows port

Optra is Linux and macOS today. A Windows port is planned for v2 and **we would
welcome help.** It is a well-defined piece of work, and the known blockers are:

1. **SSH agent access.** `src/remote.rs` calls
   `russh::keys::agent::client::AgentClient::connect_env()`, which is
   `#[cfg(unix)]` — it expects a Unix socket from `SSH_AUTH_SOCK`. Windows needs
   `connect_named_pipe` for OpenSSH, and possibly `connect_pageant` for PuTTY.
   This is a hard compile error, not a runtime failure.
2. **Crypto backend.** russh defaults to `aws-lc-rs`, which is C and assembly and
   needs NASM on Windows. russh's `ring` feature is likely the easier path.
3. **Feature gating.** `eframe`'s `wayland`, `x11` and `android-game-activity`
   features are enabled unconditionally in `Cargo.toml` and should be
   target-gated.
4. **Packaging.** `scripts/bundle.sh` is macOS-only. Windows would ship a plain
   `.exe`; file associations there are registry-based rather than
   `Info.plist`-based.

Graphics should not be a problem: `wgpu`'s default features already include
DX12, Vulkan and GLES.

If you would like to take this on, please open an issue to say so.

## Development

```bash
cargo test          # sequence ordering, theme colours, EXR decoding
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

CI runs the above on macOS and Linux. Note that GitHub runners have no usable
GPU, so **CI verifies that the code compiles and that its logic is correct, but
not that rendering works.** Test the display path on real hardware.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option. This is the Rust ecosystem's conventional dual licensing.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
