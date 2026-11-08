# Patina

A lighter Windows desktop, built piece by piece in Rust: native rendering, no
webviews, no telemetry. Two parts so far.

## Patina Launcher

Replaces Start and Windows Search with one keyboard-first window.

- **Opens with** the Win key, Win+S, Alt+Space, or a taskbar's Start button,
  always in the bottom-left corner. Win+Shift+S and every other Win+key
  shortcut still go to Windows. Ctrl+Esc opens the real Start when you need it.
- **Finds** apps (the same catalogue as Start, desktop and Store apps), settings
  pages (by keyword: "volume" finds Sound) and files (through
  [Everything](https://www.voidtools.com/), which must be running).
- **Runs** the selection with Enter, as administrator with Ctrl+Shift+Enter,
  or from the right-click menu. Ctrl+Enter shows a file in its folder.
- **Power**: lock, sleep, restart, shut down, sign out. Anything that ends the
  session takes two clicks, or Enter twice from search.
- Shows frequently used apps, quick settings and all apps A-Z when empty.

Built with [egui](https://github.com/emilk/egui) on OpenGL (wgpu measured 3-6x
the memory and start time for the same window). About 85 MB, 0% CPU while
hidden, and it appears a few milliseconds after the key press.

### Build and run

Windows 11, Rust (the toolchain is pinned in `rust-toolchain.toml`).

```
cargo build --release
target\release\patina-launcher.exe              start (or show the running one)
target\release\patina-launcher.exe --install    start hidden at every sign-in
target\release\patina-launcher.exe --uninstall  stop doing that
target\release\patina-launcher.exe --quit       close the running one
```

`--no-win-key` leaves the Win key and Start to Windows (Alt+Space still works).
A tiny helper process (`--hook`) holds the keyboard hook: Windows does not call
a process's own low-level hook for keys aimed at its own window.

Set `PATINA_TRACE=1` before starting to log input and focus decisions to
`%LOCALAPPDATA%\Patina\launcher.log`.

## WinTune

`wintune/wintune.ps1`, a reversible clean-up of an existing Windows install
(run from an elevated PowerShell):

- rarely used services start on demand instead of at boot,
- telemetry scheduled tasks are disabled,
- web results in search, Widgets, the Settings home page and the taskbar
  search box are turned off by policy.

```
wintune.ps1 -Mode Report    # show the current state, change nothing
wintune.ps1 -Mode Apply     # save the original state, then apply
wintune.ps1 -Mode Restore   # put back exactly what was saved
```

The original state is saved to `wintune/backup.json` next to the script and is
needed by Restore. It describes your machine, so it is git-ignored, along with
the run logs. The list of services in the script is tuned to one machine - read
it before applying.

## License

MIT - see [LICENSE](LICENSE).
