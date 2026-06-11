#![windows_subsystem = "windows"]
// EnergyFlag — Windows power-plan profile switcher, tray-only.
//
// Windows power plans are a maze of nested settings, and what you actually want is binary:
// either the machine must stay reachable (remote access, downloads, servers) or you're at
// the desk and it should save energy like a normal PC. EnergyFlag overwrites the active
// power plan with one of two profiles, chosen from the tray:
//
//   Remote Mode  — never sleep, never hibernate (AC and DC); only the display turns off.
//                  The machine stays reachable via AnyDesk/RDP/SSH around the clock.
//   On-Site Mode — sensible energy-saving defaults: display off soon, sleep after a while.
//
// Sibling of DeskFlag and KeyFlag (same tray / message-loop / registry / About /
// auto-update patterns) — the tray badge is drawn with plain GDI, like KeyFlag.

use core::ffi::c_void;
use std::os::windows::process::CommandExt;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::Urlmon::URLDownloadToFileW;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// ---------- Constants ----------
const WM_TRAY: u32 = WM_APP + 1;

const ID_REMOTE: usize = 1;
const ID_ONSITE: usize = 2;
const ID_ABOUT: usize = 12;
const ID_UPDATE: usize = 13;
const ID_EXIT: usize = 11;

const REG_SUBKEY: &str = "Software\\EnergyFlag";
const ABOUT_URL: &str = "https://github.com/gabrielchaves6/energyflag";
// Public releases repo (this repo is public, so releases/latest is readable unauthenticated).
const UPDATE_REPO: &str = "gabrielchaves6/energyflag";

// CREATE_NO_WINDOW — powercfg runs without flashing a console (we're a windows-subsystem app).
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Remote,
    OnSite,
}

// Timeouts (minutes, 0 = never) applied to the ACTIVE power plan: powercfg /change works
// per-user with no elevation, and overwrites whichever plan is current — exactly the intent.
//                          (monitor-ac, monitor-dc, standby-ac, standby-dc, hibernate-ac, hibernate-dc)
const REMOTE_TIMEOUTS: [(&str, u32); 6] = [
    ("monitor-timeout-ac", 30),
    ("monitor-timeout-dc", 15),
    ("standby-timeout-ac", 0),
    ("standby-timeout-dc", 0),
    ("hibernate-timeout-ac", 0),
    ("hibernate-timeout-dc", 0),
];
const ONSITE_TIMEOUTS: [(&str, u32); 6] = [
    ("monitor-timeout-ac", 10),
    ("monitor-timeout-dc", 5),
    ("standby-timeout-ac", 30),
    ("standby-timeout-dc", 20),
    ("hibernate-timeout-ac", 0),
    ("hibernate-timeout-dc", 180),
];

// ---------- Globals (single-threaded; touched only from the message thread) ----------
static mut MODE: Mode = Mode::Remote;
static mut TRAY_ICON: isize = 0; // current tray HICON (RM/OS badge, rebuilt on mode change)

static mut ABOUT_HWND: Option<HWND> = None;
static mut ABOUT_ICON: isize = 0; // brand logo (embedded energyflag.ico, or GDI fallback)
static mut ABOUT_LINK: RECT = RECT { left: 0, top: 0, right: 0, bottom: 0 };
static mut ABOUT_CLOSE: RECT = RECT { left: 0, top: 0, right: 0, bottom: 0 };
static mut ABOUT_CLOSE_HOT: bool = false;
static mut ACTIVE_WORK: RECT = RECT { left: 0, top: 0, right: 0, bottom: 0 };

static mut DLG_CLOSE: RECT = RECT { left: 0, top: 0, right: 0, bottom: 0 };
static mut DLG_CLOSE_HOT: bool = false;
static mut DLG_HEADING: String = String::new();
static mut DLG_BODY: String = String::new();
static mut DLG_PRIMARY: String = String::new();
static mut DLG_SECONDARY: String = String::new();
static mut DLG_BTN_PRIMARY: RECT = RECT { left: 0, top: 0, right: 0, bottom: 0 };
static mut DLG_BTN_SECONDARY: RECT = RECT { left: 0, top: 0, right: 0, bottom: 0 };
static mut DLG_RESULT: i32 = 0;
static mut DLG_CLASS_REGISTERED: bool = false;

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// UTF-16 without a NUL terminator (for DrawTextW / GetTextExtentPoint32W).
fn wn(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

// ---------- Persisted choice ----------
fn read_mode() -> Mode {
    unsafe {
        let sk = w(REG_SUBKEY);
        let v = w("Mode");
        let mut buf = [0u16; 16];
        let mut size = (buf.len() * 2) as u32;
        let rc = RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(sk.as_ptr()),
            PCWSTR(v.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        );
        if rc != ERROR_SUCCESS {
            return Mode::Remote;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        if String::from_utf16_lossy(&buf[..len]) == "ONSITE" {
            Mode::OnSite
        } else {
            Mode::Remote
        }
    }
}

fn write_mode(mode: Mode) {
    unsafe {
        let sk = w(REG_SUBKEY);
        let mut hkey = HKEY::default();
        let rc = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(sk.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        );
        if rc != ERROR_SUCCESS {
            return;
        }
        let val = match mode {
            Mode::Remote => "REMOTE",
            Mode::OnSite => "ONSITE",
        };
        let data = w(val);
        let bytes = core::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2);
        let _ = RegSetValueExW(hkey, PCWSTR(w("Mode").as_ptr()), 0, REG_SZ, Some(bytes));
        let _ = RegCloseKey(hkey);
    }
}

// ---------- Power plan enforcement ----------
// Run powercfg with the given args, hidden. Returns true on exit code 0.
fn powercfg(args: &[&str]) -> bool {
    std::process::Command::new("powercfg")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// Overwrite the active power plan with the chosen profile's timeouts. /change edits the
// current plan in place (per-user, no admin), so the profile survives in the Settings UI
// and across reboots — EnergyFlag only needs to run again to *switch*, not to *keep* it.
fn apply_profile(mode: Mode) {
    let timeouts: &[(&str, u32)] = match mode {
        Mode::Remote => &REMOTE_TIMEOUTS,
        Mode::OnSite => &ONSITE_TIMEOUTS,
    };
    for (setting, minutes) in timeouts {
        let m = minutes.to_string();
        powercfg(&["/change", setting, &m]);
    }
}

unsafe fn apply_mode(hwnd: HWND, mode: Mode) {
    MODE = mode;
    write_mode(mode);
    apply_profile(mode);
    refresh_tray(hwnd);
}

// ---------- GDI-drawn icons (tray badge "RM"/"OS"; brand fallback "E") ----------
unsafe fn make_text_icon(bg: COLORREF, label: &str) -> HICON {
    let sz = 32i32;
    let screen_dc = GetDC(None);
    let mem_dc = CreateCompatibleDC(screen_dc);
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: core::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: sz,
            biHeight: -sz, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = core::ptr::null_mut();
    let dib =
        CreateDIBSection(mem_dc, &mut bmi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or_default();
    let old = SelectObject(mem_dc, dib);

    let full = RECT { left: 0, top: 0, right: sz, bottom: sz };
    let brush = CreateSolidBrush(bg);
    FillRect(mem_dc, &full, brush);
    let _ = DeleteObject(brush);

    let font = make_font(20, 700, false);
    let old_font = SelectObject(mem_dc, font);
    SetBkMode(mem_dc, TRANSPARENT);
    let _ = SetTextColor(mem_dc, rgb(255, 255, 255));
    let mut txt = wn(label);
    let mut tr = full;
    DrawTextW(mem_dc, &mut txt, &mut tr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    SelectObject(mem_dc, old_font);
    let _ = DeleteObject(font);

    // GDI never writes the alpha channel, so force the whole 32bpp bitmap opaque — otherwise
    // the icon comes out fully transparent (invisible).
    let p = bits as *mut u8;
    for i in 0..(sz * sz) as usize {
        *p.add(i * 4 + 3) = 255;
    }

    SelectObject(mem_dc, old);
    let mask_bits = vec![0u8; (sz * sz) as usize];
    let mask = CreateBitmap(sz, sz, 1, 1, Some(mask_bits.as_ptr() as *const _));
    let ii = ICONINFO {
        fIcon: TRUE,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: dib,
    };
    let hicon = CreateIconIndirect(&ii).unwrap_or_default();
    let _ = DeleteObject(mask);
    let _ = DeleteObject(dib);
    let _ = DeleteDC(mem_dc);
    ReleaseDC(None, screen_dc);
    hicon
}

unsafe fn mode_icon(mode: Mode) -> HICON {
    match mode {
        // Both modes use DeskFlag blue (#2563EB) — only the label differs.
        Mode::Remote => make_text_icon(rgb(37, 99, 235), "RM"),
        Mode::OnSite => make_text_icon(rgb(37, 99, 235), "OS"),
    }
}

// Brand icon (taskbar / About / installer): the embedded energyflag.ico (MSVC build) if
// present, else an energyflag.ico next to the exe, else a GDI-drawn DeskFlag-blue "E".
unsafe fn load_app_icon() -> HICON {
    let hinst: HINSTANCE = GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default();
    if let Ok(h) = LoadIconW(hinst, PCWSTR(1 as *const u16)) {
        if !h.is_invalid() {
            return h;
        }
    }
    if let Some(h) = icon_from_file() {
        return h;
    }
    make_text_icon(rgb(37, 99, 235), "E")
}

unsafe fn icon_from_file() -> Option<HICON> {
    let mut buf = [0u16; 260];
    let n = GetModuleFileNameW(None, &mut buf);
    if n == 0 {
        return None;
    }
    let exe = String::from_utf16_lossy(&buf[..n as usize]);
    let ico = std::path::Path::new(&exe).parent()?.join("energyflag.ico");
    if !ico.exists() {
        return None;
    }
    let icow = w(&ico.to_string_lossy());
    let h = LoadImageW(None, PCWSTR(icow.as_ptr()), IMAGE_ICON, 0, 0, LR_LOADFROMFILE | LR_DEFAULTSIZE).ok()?;
    Some(HICON(h.0))
}

// ---------- Tray ----------
unsafe fn tray_base(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW::default();
    nid.cbSize = core::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid
}

unsafe fn tooltip() -> &'static str {
    match MODE {
        Mode::Remote => "EnergyFlag — Remote Mode (nunca dorme)",
        Mode::OnSite => "EnergyFlag — On-Site Mode (economia)",
    }
}

unsafe fn add_tray(hwnd: HWND) {
    let icon = mode_icon(MODE);
    TRAY_ICON = icon.0 as isize;
    let mut nid = tray_base(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = icon;
    let tip = w(tooltip());
    let n = tip.len().min(127);
    nid.szTip[..n].copy_from_slice(&tip[..n]);
    let _ = Shell_NotifyIconW(NIM_ADD, &nid);
}

unsafe fn refresh_tray(hwnd: HWND) {
    let icon = mode_icon(MODE);
    let mut nid = tray_base(hwnd);
    nid.uFlags = NIF_ICON | NIF_TIP;
    nid.hIcon = icon;
    let tip = w(tooltip());
    let n = tip.len().min(127);
    nid.szTip[..n].copy_from_slice(&tip[..n]);
    let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    if TRAY_ICON != 0 {
        let _ = DestroyIcon(HICON(TRAY_ICON as *mut c_void));
    }
    TRAY_ICON = icon.0 as isize;
}

unsafe fn remove_tray(hwnd: HWND) {
    let nid = tray_base(hwnd);
    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
}

unsafe fn show_tray_menu(hwnd: HWND) {
    let menu = CreatePopupMenu().unwrap_or_default();
    let _ = AppendMenuW(menu, MF_STRING, ID_REMOTE, PCWSTR(w("Remote Mode — nunca dorme").as_ptr()));
    let _ = AppendMenuW(menu, MF_STRING, ID_ONSITE, PCWSTR(w("On-Site Mode — economia").as_ptr()));
    let active = if MODE == Mode::Remote { ID_REMOTE } else { ID_ONSITE } as u32;
    let _ = CheckMenuRadioItem(menu, ID_REMOTE as u32, ID_ONSITE as u32, active, MF_BYCOMMAND.0);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_ABOUT, PCWSTR(w("About EnergyFlag").as_ptr()));
    let _ = AppendMenuW(menu, MF_STRING, ID_UPDATE, PCWSTR(w("Check for updates…").as_ptr()));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, PCWSTR(w("Exit").as_ptr()));

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD, pt.x, pt.y, 0, hwnd, None);
    let _ = DestroyMenu(menu);
    match cmd.0 as usize {
        ID_REMOTE => apply_mode(hwnd, Mode::Remote),
        ID_ONSITE => apply_mode(hwnd, Mode::OnSite),
        ID_ABOUT => {
            set_active_work();
            show_about();
        }
        ID_UPDATE => {
            set_active_work();
            check_for_updates(hwnd);
        }
        ID_EXIT => {
            remove_tray(hwnd);
            PostQuitMessage(0);
        }
        _ => {}
    }
}

unsafe fn set_active_work() {
    let mut work = RECT::default();
    let _ = SystemParametersInfoW(
        SPI_GETWORKAREA,
        0,
        Some(&mut work as *mut _ as *mut c_void),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    );
    ACTIVE_WORK = work;
}

// ===================== Auto-update (pull from the public releases repo) =====================
fn ver_tuple(s: &str) -> (u32, u32, u32) {
    let s = s.trim().trim_start_matches('v');
    let mut p = s.split(|c| c == '.' || c == '-' || c == '+');
    let n = |o: Option<&str>| o.and_then(|x| x.trim().parse::<u32>().ok()).unwrap_or(0);
    (n(p.next()), n(p.next()), n(p.next()))
}

fn json_string(body: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let after = &body[body.find(&pat)? + pat.len()..];
    let after = &after[after.find(':')? + 1..];
    let after = &after[after.find('"')? + 1..];
    Some(after[..after.find('"')?].to_string())
}

fn find_exe_asset(body: &str) -> Option<String> {
    let key = "\"browser_download_url\"";
    let mut start = 0;
    while let Some(i) = body[start..].find(key) {
        let abs = start + i;
        if let Some(u) = json_string(&body[abs..], "browser_download_url") {
            if u.to_lowercase().ends_with(".exe") {
                return Some(u);
            }
        }
        start = abs + key.len();
    }
    None
}

unsafe fn url_download(url: &str, dest: &std::path::Path) -> bool {
    let url_w = w(url);
    let dest_w = w(&dest.to_string_lossy());
    URLDownloadToFileW(None, PCWSTR(url_w.as_ptr()), PCWSTR(dest_w.as_ptr()), 0, None).is_ok()
}

unsafe fn check_for_updates(hwnd: HWND) {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let api = format!("https://api.github.com/repos/{UPDATE_REPO}/releases/latest?_={nonce}");
    let tmp = std::env::temp_dir();
    let json_path = tmp.join("energyflag_release.json");
    if !url_download(&api, &json_path) {
        show_dialog("Sem conexão", "Não foi possível verificar atualizações. Verifique sua conexão com a internet e tente novamente.", "OK", "");
        return;
    }
    let body = std::fs::read_to_string(&json_path).unwrap_or_default();
    let _ = std::fs::remove_file(&json_path);

    let latest = json_string(&body, "tag_name").unwrap_or_default();
    let cur = env!("CARGO_PKG_VERSION");
    if latest.is_empty() {
        show_dialog("Não foi possível verificar", "Não foi possível ler a versão mais recente do servidor.", "OK", "");
        return;
    }
    if ver_tuple(&latest) <= ver_tuple(cur) {
        show_dialog("Tudo atualizado", &format!("Você já está na versão mais recente ({latest})."), "OK", "");
        return;
    }
    let Some(url) = find_exe_asset(&body) else {
        show_dialog("Atualização disponível", &format!("A versão {latest} está disponível, mas o instalador não foi encontrado no release."), "OK", "");
        return;
    };

    let prompt = format!("A versão {latest} está disponível (você tem v{cur}).\n\nAtualizar agora? O EnergyFlag será reiniciado automaticamente — sem assistente de instalação.");
    if !show_dialog("Atualização disponível", &prompt, "Atualizar agora", "Depois") {
        return;
    }
    let setup = tmp.join("EnergyFlag-Setup.exe");
    if !url_download(&url, &setup) {
        show_dialog("Falha no download", "Não foi possível baixar o instalador. Tente novamente mais tarde.", "OK", "");
        return;
    }
    // Run the installer silently and quit so it can replace the running EnergyFlag.exe. Setup
    // kills this instance (KillRunning), installs over it, then its [Run] entry relaunches the
    // new build — so the lone consent click above is the whole update, no wizard.
    let setup_w = w(&setup.to_string_lossy());
    let args_w = w("/VERYSILENT /SUPPRESSMSGBOXES /NORESTART");
    ShellExecuteW(hwnd, PCWSTR(w("open").as_ptr()), PCWSTR(setup_w.as_ptr()), PCWSTR(args_w.as_ptr()), PCWSTR::null(), SW_SHOWNORMAL);
    remove_tray(hwnd);
    PostQuitMessage(0);
}

// ===================== Borderless window chrome (custom close + drag) =====================
unsafe fn make_font(height: i32, weight: i32, underline: bool) -> HFONT {
    CreateFontW(
        -height,
        0,
        0,
        0,
        weight,
        0,
        if underline { 1 } else { 0 },
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        (DEFAULT_PITCH.0 | (FF_DONTCARE.0 << 4) as u8) as u32,
        PCWSTR(w("Segoe UI").as_ptr()),
    )
}

const CLOSE_BTN_W: i32 = 46;
const CLOSE_BTN_H: i32 = 36;
const DRAG_STRIP_H: i32 = 92;

fn in_rect(r: RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn close_btn_rect(client_right: i32) -> RECT {
    RECT { left: client_right - CLOSE_BTN_W, top: 0, right: client_right, bottom: CLOSE_BTN_H }
}

unsafe fn paint_close_btn(hdc: HDC, client_right: i32, hot: bool) -> RECT {
    let rc = close_btn_rect(client_right);
    if hot {
        let hb = CreateSolidBrush(rgb(196, 43, 43));
        FillRect(hdc, &rc, hb);
        let _ = DeleteObject(hb);
    }
    let cx = (rc.left + rc.right) / 2;
    let cy = (rc.top + rc.bottom) / 2;
    let s = 5;
    let pen = CreatePen(PS_SOLID, 1, if hot { rgb(255, 255, 255) } else { rgb(180, 186, 198) });
    let old = SelectObject(hdc, pen);
    let _ = MoveToEx(hdc, cx - s, cy - s, None);
    let _ = LineTo(hdc, cx + s + 1, cy + s + 1);
    let _ = MoveToEx(hdc, cx + s, cy - s, None);
    let _ = LineTo(hdc, cx - s - 1, cy + s + 1);
    SelectObject(hdc, old);
    let _ = DeleteObject(pen);
    rc
}

unsafe fn setup_chrome(hwnd: HWND) {
    let _ = SendMessageW(hwnd, WM_SETICON, WPARAM(0), LPARAM(ABOUT_ICON)); // ICON_SMALL
    let _ = SendMessageW(hwnd, WM_SETICON, WPARAM(1), LPARAM(ABOUT_ICON)); // ICON_BIG
    let round: i32 = 2; // DWMWCP_ROUND
    let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(33), &round as *const _ as *const _, 4);
}

unsafe fn begin_drag_if_top(hwnd: HWND, x: i32, y: i32, client_right: i32) -> bool {
    if y < DRAG_STRIP_H && !in_rect(close_btn_rect(client_right), x, y) {
        let _ = ReleaseCapture();
        SendMessageW(hwnd, WM_NCLBUTTONDOWN, WPARAM(HTCAPTION as usize), LPARAM(0));
        return true;
    }
    false
}

// ===================== About window =====================
extern "system" fn about_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);

                let bg = CreateSolidBrush(rgb(24, 27, 35));
                FillRect(hdc, &rc, bg);
                let _ = DeleteObject(bg);

                let pad = 28;
                if ABOUT_ICON != 0 {
                    let _ = DrawIconEx(hdc, pad, 34, HICON(ABOUT_ICON as *mut _), 56, 56, 0, None, DI_NORMAL);
                }

                SetBkMode(hdc, TRANSPARENT);
                let title_font = make_font(27, 600, false);
                let body_font = make_font(17, 400, false);
                let link_font = make_font(17, 400, true);

                let text_x = pad + 56 + 18;
                let old = SelectObject(hdc, title_font);
                let _ = SetTextColor(hdc, rgb(250, 250, 252));
                let mut r = RECT { left: text_x, top: 36, right: rc.right - pad, bottom: 68 };
                let mut t = wn("EnergyFlag");
                DrawTextW(hdc, &mut t, &mut r, DT_LEFT | DT_SINGLELINE);

                SelectObject(hdc, body_font);
                let _ = SetTextColor(hdc, rgb(150, 156, 170));
                let mut r2 = RECT { left: text_x, top: 70, right: rc.right - pad, bottom: 94 };
                let mut t2 = wn(&format!("Version {}", env!("CARGO_PKG_VERSION")));
                DrawTextW(hdc, &mut t2, &mut r2, DT_LEFT | DT_SINGLELINE);

                let _ = SetTextColor(hdc, rgb(196, 202, 214));
                let mut r3 = RECT { left: pad, top: 116, right: rc.right - pad, bottom: 144 };
                let mut t3 = wn("Power-plan profile switcher");
                DrawTextW(hdc, &mut t3, &mut r3, DT_LEFT | DT_SINGLELINE);

                let _ = SetTextColor(hdc, rgb(150, 156, 170));
                let mut r4 = RECT { left: pad, top: 146, right: rc.right - pad, bottom: 174 };
                let mut t4 = wn("Remote: nunca dorme. On-Site: economia de energia.");
                DrawTextW(hdc, &mut t4, &mut r4, DT_LEFT | DT_SINGLELINE);

                SelectObject(hdc, link_font);
                let _ = SetTextColor(hdc, rgb(90, 150, 245));
                let link_top = rc.bottom - 40;
                let mut r5 = RECT { left: pad, top: link_top, right: rc.right - pad, bottom: rc.bottom - 14 };
                let mut t5 = wn("github.com/gabrielchaves6/energyflag");
                DrawTextW(hdc, &mut t5, &mut r5, DT_LEFT | DT_SINGLELINE);
                let mut sz = SIZE::default();
                let _ = GetTextExtentPoint32W(hdc, &t5, &mut sz);
                ABOUT_LINK = RECT { left: pad, top: link_top, right: pad + sz.cx, bottom: link_top + sz.cy };

                SelectObject(hdc, old);
                let _ = DeleteObject(title_font);
                let _ = DeleteObject(body_font);
                let _ = DeleteObject(link_font);
                ABOUT_CLOSE = paint_close_btn(hdc, rc.right, ABOUT_CLOSE_HOT);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                begin_drag_if_top(hwnd, x, y, rc.right);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                if in_rect(ABOUT_CLOSE, x, y) {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                } else if in_rect(ABOUT_LINK, x, y) {
                    let _ = ShellExecuteW(
                        None,
                        PCWSTR(w("open").as_ptr()),
                        PCWSTR(w(ABOUT_URL).as_ptr()),
                        PCWSTR::null(),
                        PCWSTR::null(),
                        SW_SHOWNORMAL,
                    );
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                let hot = in_rect(ABOUT_CLOSE, x, y);
                if hot != ABOUT_CLOSE_HOT {
                    ABOUT_CLOSE_HOT = hot;
                    let _ = InvalidateRect(hwnd, Some(&ABOUT_CLOSE), FALSE);
                }
                LRESULT(0)
            }
            WM_SETCURSOR => {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(hwnd, &mut pt);
                if in_rect(ABOUT_LINK, pt.x, pt.y) || in_rect(ABOUT_CLOSE, pt.x, pt.y) {
                    if let Ok(hand) = LoadCursorW(None, IDC_HAND) {
                        SetCursor(hand);
                    }
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_KEYDOWN if VIRTUAL_KEY(wparam.0 as u16) == VK_ESCAPE => {
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_GETICON => LRESULT(ABOUT_ICON),
            WM_CLOSE => {
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

unsafe fn show_about() {
    let hwnd = match ABOUT_HWND {
        Some(h) => h,
        None => return,
    };
    let w_px = 440;
    let h_px = 260;
    let work = ACTIVE_WORK;
    let (cx, cy) = if work.right > work.left {
        (work.left + (work.right - work.left) / 2, work.top + (work.bottom - work.top) / 2)
    } else {
        (GetSystemMetrics(SM_CXSCREEN) / 2, GetSystemMetrics(SM_CYSCREEN) / 2)
    };
    let _ = SetWindowPos(hwnd, HWND_TOPMOST, cx - w_px / 2, cy - h_px / 2, w_px, h_px, SWP_SHOWWINDOW);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    let _ = InvalidateRect(hwnd, None, TRUE);
}

// ===================== Styled message dialog (About look) =====================
const DLG_W: i32 = 430;
const DLG_H: i32 = 248; // minimum dialog height (grown to fit the body)
const DLG_BTN_W: i32 = 116; // minimum button width
const DLG_BTN_H: i32 = 34;
const DLG_BTN_PAD_X: i32 = 18; // horizontal text padding inside a button
const DLG_BODY_GAP: i32 = 28; // vertical gap between the body text and the buttons

// The font used for button labels — shared by measuring and painting so widths match.
unsafe fn button_font() -> HFONT {
    make_font(17, 600, false)
}

// Width a button needs to fit `label`: text extent plus padding, never below the minimum.
unsafe fn button_width(hdc: HDC, label: &str) -> i32 {
    let font = button_font();
    let of = SelectObject(hdc, font);
    let mut t = wn(label);
    let mut r = RECT::default();
    DrawTextW(hdc, &mut t, &mut r, DT_CALCRECT | DT_SINGLELINE);
    SelectObject(hdc, of);
    let _ = DeleteObject(font);
    (r.right - r.left + 2 * DLG_BTN_PAD_X).max(DLG_BTN_W)
}

// Paint one flat, slightly-rounded button `width` wide and return its rect (for hit-testing).
unsafe fn paint_button(hdc: HDC, x: i32, y: i32, width: i32, label: &str, accent: bool) -> RECT {
    let rc = RECT { left: x, top: y, right: x + width, bottom: y + DLG_BTN_H };
    let fill = CreateSolidBrush(if accent { rgb(56, 118, 240) } else { rgb(48, 52, 62) });
    let pen = CreatePen(PS_SOLID, 1, if accent { rgb(56, 118, 240) } else { rgb(74, 80, 92) });
    let old_b = SelectObject(hdc, fill);
    let old_p = SelectObject(hdc, pen);
    let _ = RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, 12, 12);
    SelectObject(hdc, old_b);
    SelectObject(hdc, old_p);
    let _ = DeleteObject(fill);
    let _ = DeleteObject(pen);

    let font = make_font(17, 600, false);
    let of = SelectObject(hdc, font);
    SetBkMode(hdc, TRANSPARENT);
    let _ = SetTextColor(hdc, if accent { rgb(255, 255, 255) } else { rgb(214, 219, 228) });
    let mut t = wn(label);
    let mut r = rc;
    DrawTextW(hdc, &mut t, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    SelectObject(hdc, of);
    let _ = DeleteObject(font);
    rc
}

extern "system" fn dialog_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);

                let bg = CreateSolidBrush(rgb(24, 27, 35));
                FillRect(hdc, &rc, bg);
                let _ = DeleteObject(bg);

                let pad = 28;
                if ABOUT_ICON != 0 {
                    let _ = DrawIconEx(hdc, pad, 30, HICON(ABOUT_ICON as *mut _), 48, 48, 0, None, DI_NORMAL);
                }

                SetBkMode(hdc, TRANSPARENT);
                let title_font = make_font(24, 600, false);
                let body_font = make_font(17, 400, false);

                let text_x = pad + 48 + 16;
                let old = SelectObject(hdc, title_font);
                let _ = SetTextColor(hdc, rgb(250, 250, 252));
                let mut r = RECT { left: text_x, top: 38, right: rc.right - pad, bottom: 74 };
                let mut t = wn(&DLG_HEADING);
                DrawTextW(hdc, &mut t, &mut r, DT_LEFT | DT_SINGLELINE);

                SelectObject(hdc, body_font);
                let _ = SetTextColor(hdc, rgb(196, 202, 214));
                let mut r2 = RECT { left: pad, top: 96, right: rc.right - pad, bottom: rc.bottom - 64 };
                let mut t2 = wn(&DLG_BODY);
                DrawTextW(hdc, &mut t2, &mut r2, DT_LEFT | DT_WORDBREAK);
                SelectObject(hdc, old);
                let _ = DeleteObject(title_font);
                let _ = DeleteObject(body_font);

                // Buttons, bottom-right. Each is sized to its label so a longer label like
                // "Atualizar agora" never overflows the button.
                let by = rc.bottom - 20 - DLG_BTN_H;
                let pw = button_width(hdc, &DLG_PRIMARY);
                let px = rc.right - pad - pw;
                DLG_BTN_PRIMARY = paint_button(hdc, px, by, pw, &DLG_PRIMARY, true);
                if !DLG_SECONDARY.is_empty() {
                    let sw = button_width(hdc, &DLG_SECONDARY);
                    let sx = px - 12 - sw;
                    DLG_BTN_SECONDARY = paint_button(hdc, sx, by, sw, &DLG_SECONDARY, false);
                } else {
                    DLG_BTN_SECONDARY = RECT::default();
                }

                DLG_CLOSE = paint_close_btn(hdc, rc.right, DLG_CLOSE_HOT);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                begin_drag_if_top(hwnd, x, y, rc.right);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                if in_rect(DLG_CLOSE, x, y) {
                    DLG_RESULT = 2;
                    let _ = DestroyWindow(hwnd);
                } else if in_rect(DLG_BTN_PRIMARY, x, y) {
                    DLG_RESULT = 1;
                    let _ = DestroyWindow(hwnd);
                } else if !DLG_SECONDARY.is_empty() && in_rect(DLG_BTN_SECONDARY, x, y) {
                    DLG_RESULT = 2;
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                let hot = in_rect(DLG_CLOSE, x, y);
                if hot != DLG_CLOSE_HOT {
                    DLG_CLOSE_HOT = hot;
                    let _ = InvalidateRect(hwnd, Some(&DLG_CLOSE), FALSE);
                }
                LRESULT(0)
            }
            WM_SETCURSOR => {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(hwnd, &mut pt);
                if in_rect(DLG_BTN_PRIMARY, pt.x, pt.y)
                    || in_rect(DLG_CLOSE, pt.x, pt.y)
                    || (!DLG_SECONDARY.is_empty() && in_rect(DLG_BTN_SECONDARY, pt.x, pt.y))
                {
                    if let Ok(hand) = LoadCursorW(None, IDC_HAND) {
                        SetCursor(hand);
                    }
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_KEYDOWN => {
                match VIRTUAL_KEY(wparam.0 as u16) {
                    VK_RETURN => {
                        DLG_RESULT = 1;
                        let _ = DestroyWindow(hwnd);
                    }
                    VK_ESCAPE => {
                        DLG_RESULT = 2;
                        let _ = DestroyWindow(hwnd);
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_GETICON => LRESULT(ABOUT_ICON),
            WM_CLOSE => {
                if DLG_RESULT == 0 {
                    DLG_RESULT = 2;
                }
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

// Show a modal styled dialog and return true iff the primary button was chosen. `secondary`
// "" means a single (OK-style) button. Runs a nested message loop until the dialog closes.
unsafe fn show_dialog(heading: &str, body: &str, primary: &str, secondary: &str) -> bool {
    let hinstance: HINSTANCE = GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default();
    let class = w("EnergyFlagDialog");
    if !DLG_CLASS_REGISTERED {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(dialog_wndproc),
            hInstance: hinstance,
            lpszClassName: PCWSTR(class.as_ptr()),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: HICON(ABOUT_ICON as *mut _),
            ..Default::default()
        };
        RegisterClassW(&wc);
        DLG_CLASS_REGISTERED = true;
    }

    DLG_HEADING = heading.to_string();
    DLG_BODY = body.to_string();
    DLG_PRIMARY = primary.to_string();
    DLG_SECONDARY = secondary.to_string();
    DLG_RESULT = 0;

    // Grow the dialog vertically to fit the word-wrapped body, keeping a fixed gap above the
    // buttons — so a long prompt no longer collides with them. Floors at DLG_H.
    let dlg_h = {
        let screen = GetDC(None);
        let font = make_font(17, 400, false);
        let of = SelectObject(screen, font);
        let mut t = wn(body);
        let mut r = RECT { left: 0, top: 0, right: DLG_W - 2 * 28, bottom: 0 };
        DrawTextW(screen, &mut t, &mut r, DT_LEFT | DT_WORDBREAK | DT_CALCRECT);
        let body_h = r.bottom - r.top;
        SelectObject(screen, of);
        let _ = DeleteObject(font);
        ReleaseDC(None, screen);
        (96 + body_h + DLG_BODY_GAP + DLG_BTN_H + 20).max(DLG_H)
    };

    let work = ACTIVE_WORK;
    let (cx, cy) = if work.right > work.left {
        (work.left + (work.right - work.left) / 2, work.top + (work.bottom - work.top) / 2)
    } else {
        (GetSystemMetrics(SM_CXSCREEN) / 2, GetSystemMetrics(SM_CYSCREEN) / 2)
    };
    let hwnd = match CreateWindowExW(
        WINDOW_EX_STYLE(0),
        PCWSTR(class.as_ptr()),
        PCWSTR(w("EnergyFlag").as_ptr()),
        WS_POPUP,
        cx - DLG_W / 2,
        cy - dlg_h / 2,
        DLG_W,
        dlg_h,
        None,
        None,
        hinstance,
        None,
    ) {
        Ok(h) => h,
        Err(_) => return false,
    };
    setup_chrome(hwnd);
    DLG_CLOSE_HOT = false;

    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);

    let mut m = MSG::default();
    loop {
        if DLG_RESULT != 0 {
            break;
        }
        if !GetMessageW(&mut m, None, 0, 0).as_bool() {
            break;
        }
        let _ = TranslateMessage(&m);
        DispatchMessageW(&m);
    }
    if IsWindow(hwnd).as_bool() {
        let _ = DestroyWindow(hwnd);
    }
    DLG_RESULT == 1
}

// ---------- Window proc (hidden message window) ----------
extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            WM_TRAY => {
                let m = (lparam.0 as u32) & 0xffff;
                if m == WM_LBUTTONUP || m == WM_RBUTTONUP || m == WM_CONTEXTMENU {
                    show_tray_menu(hwnd);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn main() -> Result<()> {
    unsafe {
        // Single instance: two switchers would fight over the power plan.
        let name = w("EnergyFlag_singleton_mutex");
        let _mutex = CreateMutexW(None, TRUE, PCWSTR(name.as_ptr()));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }

        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        MODE = read_mode();

        let app_icon = load_app_icon();
        ABOUT_ICON = app_icon.0 as isize;

        let hinstance = GetModuleHandleW(None)?;
        let class_name = w("EnergyFlagWnd");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&wc);

        // Hidden message window: receives the tray callback.
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(w("EnergyFlag").as_ptr()),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            hinstance,
            None,
        )?;

        // About window: a real, interactive borderless window with the EnergyFlag icon.
        let about_class = w("EnergyFlagAbout");
        let wc_about = WNDCLASSW {
            lpfnWndProc: Some(about_wndproc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(about_class.as_ptr()),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: app_icon,
            ..Default::default()
        };
        RegisterClassW(&wc_about);
        let about_hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(about_class.as_ptr()),
            PCWSTR(w("About EnergyFlag").as_ptr()),
            WS_POPUP,
            0,
            0,
            440,
            260,
            None,
            None,
            hinstance,
            None,
        )?;
        setup_chrome(about_hwnd);
        ABOUT_HWND = Some(about_hwnd);

        add_tray(hwnd);
        // Re-apply the persisted profile on startup, so a reinstalled Windows update or a
        // manual Settings tweak gets snapped back to the chosen mode at logon.
        apply_mode(hwnd, MODE);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
