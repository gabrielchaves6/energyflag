<p align="center">
  <img src="rs/assets/logo.png" width="120" alt="EnergyFlag logo">
</p>

<h1 align="center">EnergyFlag</h1>

<p align="center">Switch Windows between <b>two power profiles</b> — Remote or On-Site — from a tray icon.</p>

Written in Rust (`windows-rs`). A sibling of
[DeskFlag](https://github.com/gabrielchaves6/desk_flag) and
[KeyFlag](https://github.com/gabrielchaves6/keyflag).

Windows power plans are a maze of nested settings, but what you actually want is binary:
either the machine must **stay reachable** (AnyDesk/RDP/SSH, downloads, long jobs) or you're
**at the desk** and it should save energy like a normal PC. EnergyFlag overwrites the active
power plan with one of two profiles, chosen from the tray.

## What it does

- Two modes, chosen from the tray:

  | Setting | **Remote Mode** (RM) | **On-Site Mode** (OS) |
  |---|---|---|
  | Sleep (AC / battery) | Never / Never | 30 min / 20 min |
  | Hibernate (AC / battery) | Never / Never | Never / 180 min |
  | Display off (AC / battery) | 30 min / 15 min | 10 min / 5 min |

- **Remote Mode** guarantees the machine never sleeps or hibernates, so remote-access tools
  stay connected around the clock — only the display turns off.
- **On-Site Mode** restores sensible energy-saving defaults for when you're physically there.
- Settings are applied to the **active power plan** via `powercfg /change` — per-user, no
  admin prompt, and they persist in the Windows Settings UI and across reboots.
- **Aviso "EM CONTROLE REMOTO"** (optional, Remote Mode only): covers every monitor with a
  nearly-opaque pastel-red veil with **EM CONTROLE REMOTO** in the middle, so anyone in
  front of the machine sees it's being driven remotely. The veil is purely visual — every
  click falls through, it never takes focus, and it's excluded from screen capture, so the
  remote session (AnyDesk/RDP) sees the normal desktop.
- **Brilho 0% — via Power Display** (optional, Remote Mode only): asks PowerToys Power
  Display to set every monitor's brightness to 0% while you drive the machine remotely, and
  restores it when toggled off, when switching to On-Site Mode, or on exit. EnergyFlag never
  talks to the monitors itself — it only signals the named events PowerDisplay listens to
  (see *How it works*).
- **Tray-only.** No window, no taskbar entry. The tray icon *is* the indicator: a blue
  **RM** or **OS** badge tells you which profile is active at a glance.
- Re-applies the chosen profile at startup, snapping back anything Windows Update or a
  manual Settings tweak changed.
- Styled **About** window and an in-app **Check for updates** (pulls the latest installer
  from this repo's releases), matching DeskFlag and KeyFlag.
- Remembers the last mode across restarts (`HKCU\Software\EnergyFlag`).
- Single instance (a named mutex).

## Install

EnergyFlag is a single, self-contained `EnergyFlag.exe` (the MSVC runtime is statically
linked, so **no Visual C++ redistributable is needed**). It needs Windows 10/11.

### Option A — the installer (recommended)

1. Download `EnergyFlag-Setup.exe` from a
   [Release](https://github.com/gabrielchaves6/energyflag/releases).
2. Run it. It installs per-user (no admin), into `%LOCALAPPDATA%\Programs\EnergyFlag`, adds
   a Start-menu entry, and offers a *Start EnergyFlag when I sign in to Windows* checkbox.

> **Heads-up — unsigned for now.** The installer isn't code-signed yet, so Windows
> SmartScreen may show *"Windows protected your PC"* (click **More info → Run anyway**), and
> Microsoft Defender may flag the setup with a generic false positive common for unsigned
> Inno Setup installers. The `EnergyFlag.exe` itself is not flagged.

The installer is produced automatically by GitHub Actions on every version tag
(`installer/EnergyFlag.iss`, built by `.github/workflows/release.yml`).

### Option B — copy the prebuilt executable

Build it (see below) or grab `EnergyFlag.exe` from a Release, copy it anywhere, and run it.

### Start automatically with Windows

Drop a shortcut to `EnergyFlag.exe` in the Startup folder (`Win+R` → `shell:startup`).

## Usage

Run EnergyFlag. A small blue **RM**/**OS** badge appears in the tray (possibly under the `^`
overflow). Right-click it:

- **Remote Mode — nunca dorme** / **On-Site Mode — economia** — pick the profile (the
  active one is dotted).
- **Aviso na tela — EM CONTROLE REMOTO** — toggle the red screen veil (only available in
  Remote Mode; remembered across restarts).
- **Brilho 0% — via Power Display** — toggle 0% monitor brightness through PowerToys
  (Remote Mode only; remembered across restarts; requires the one-time PowerToys setup
  below).
- **About EnergyFlag** / **Check for updates…** / **Exit**

### One-time PowerToys setup for "Brilho 0%"

The brightness toggle delegates all monitor control to **PowerToys Power Display** (WMI for
laptop panels, DDC/CI for externals). PowerDisplay applies a saved profile whenever the
LightSwitch theme events fire, and EnergyFlag signals exactly those events. Setup:

1. In **PowerToys → Power Display**, create two profiles: one with every monitor's
   brightness at **0%** (e.g. *EnergyFlag Escuro*) and one with your normal brightness
   (e.g. *EnergyFlag Normal*).
2. In **PowerToys → Light Switch → Apply monitor settings to**, set the **dark mode
   profile** to the 0% one and the **light mode profile** to the normal one. The Light
   Switch *module* can stay disabled (and its schedule **Off**) — PowerDisplay reads the
   mapping regardless, and keeping it off means nothing else fires those events.

PowerToys must be running for the toggle to have any effect. If you actually use Light
Switch for automatic theme switching, skip this feature — theme transitions would also
change your brightness.

## Build from source

Requires the [Rust toolchain](https://rustup.rs). The C runtime is statically linked
(`rs/.cargo/config.toml`), so the resulting `EnergyFlag.exe` is self-contained.

```powershell
cd rs
cargo build --release
```

The result is `rs\target\release\EnergyFlag.exe`.

CI builds release binaries with the stock MSVC toolchain, which also embeds the app icon as
a resource (`rs/app.rc` → `rs/assets/energyflag.ico`) so Explorer shows it on the .exe file.
The same `.ico` is additionally compiled into the binary with `include_bytes!`, so the tray
and all windows show the real logo on every toolchain (including local GNU builds). The logo
is generated by `tools/make_logo.ps1`.

## How it works

Each profile is just six timeouts (sleep / hibernate / display, AC and DC). Switching mode
runs `powercfg /change <setting> <minutes>` for each — the documented, per-user way to edit
the **active** plan in place. Nothing is duplicated or hidden: open *Settings → System →
Power* and you'll see exactly what EnergyFlag set. The chosen mode is saved to
`HKCU\Software\EnergyFlag` and re-applied at every startup.
