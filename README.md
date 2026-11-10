# island

A Dynamic Island for Windows, for working with coding agents in WSL. Collapsed, it is a small dot: orange pulse when blocked, green when done, white spinner when working, dim white when idle. A white ring shows media, with an orbiting arc while playing. Click to jump to an agent that needs you, or expand for the agents list and media controls. Native Rust (Win32, Direct2D, DirectComposition). It uses no CPU while idle and stays under 20 MB.

A 64×64 pixel-art pet sits beside the island, on its left, whenever the island is shown. Her pose follows the agents' status in priority order: blocked > done > working > idle. She sleeps when no agents are reported.

The island always opens on Agents; the Media tab appears while media is around. The agents list scrolls with the mouse wheel when it is longer than four rows.

## Install

From WSL, with Rust and [`cargo-xwin`](https://github.com/rust-cross/cargo-xwin):

```sh
scripts/install.sh
```

This builds both programs, installs the bridge to `~/.local/bin/island-bridge` and `island.exe` to `%LOCALAPPDATA%\island\`, then starts the island (stopping any copy already running). Run it again to update. Drag the dot anywhere on the main monitor (in Attached mode, along the top edge); the spot is saved. It opens away from the edges it sits at, so at the bottom it opens upward. The tray icon (a white dot) has Hide/Show, Pill/Attached, Reset position, Start with Windows and Quit.

## Config

`%LOCALAPPDATA%\island\config.toml` is read at start. Missing keys take their defaults. The tray menu writes it back.

| Key | Default | |
|---|---|---|
| `display_mode` | `"pill"` | `"pill"` floats below the top edge; `"attached"` hangs from it |
| `position_x` | `0.5` | where the island sits across the screen: `0` far left, `0.5` centre, `1` far right; set by dragging it (older configs call it `position`) |
| `position_y` | `0.0` | and down it: `0` top, `1` bottom, just above the taskbar; set by dragging it; Attached ignores it |
| `start_with_windows` | `true` | kept in sync with `HKCU\…\Run\Island` at every start |
| `wsl_distro` | `""` | distro running herdr; `""` = the default distro |
| `bridge_path` | `"$HOME/.local/bin/island-bridge"` | bridge path inside WSL |
| `terminal_window_class` | `"CASCADIA_HOSTING_WINDOW_CLASS"` | window brought forward after a jump (Windows Terminal) |
| `terminal_process` | `""` | terminal executable, e.g. `"warp.exe"`; wins over the class when set |
| `terminal_launch` | `""` | run when a jump finds no terminal window: a URI or a program, e.g. `"warp://tab_config/herdr"` |

Logs go to `%LOCALAPPDATA%\island\island.log`. Pass `--debug` for more detail, or `--demo` for a scripted tour that saves nothing.

## How it finds things

**Agents.** The island never starts WSL. Every 15 s it checks `wsl.exe -l -v`. Once the configured distro is running, it starts `island-bridge` through `wsl.exe`. The bridge relays JSON lines between the island and herdr's socket (`$HERDR_SOCKET_PATH`, else `~/.config/herdr/herdr.sock`). The island subscribes to herdr's agent events, so a blocked agent shows within a moment. Clicking a blocked or done agent focuses its pane in herdr and brings the terminal forward. With no terminal window open, it runs `terminal_launch` instead: for Warp, a tab config whose pane runs `herdr` reattaches to the session on that pane. If WSL, the bridge or herdr is down, the island shows nothing about agents.

**Media.** The island follows Windows' current media session (SMTC), so any player that publishes there works: Spotify, browsers and the like. Controls and seeking go back through the same session.

**Fullscreen.** When the foreground window covers the whole main monitor (a video, a game), the island hides. The one exception is a blocked agent: its dot still shows.

## Develop

`cargo test --workspace --exclude island` runs the pure logic in WSL. `scripts/build.sh` builds release binaries. `target/x86_64-pc-windows-msvc/debug/island.exe` runs straight from WSL.
