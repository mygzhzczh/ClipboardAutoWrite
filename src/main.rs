#![windows_subsystem = "windows"]

use std::ffi::c_void;
use std::fs;
use std::mem::MaybeUninit;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

const CREATE_NO_WINDOW: u32 = 0x08000000;

type HWND = isize;
type WPARAM = usize;
type LPARAM = isize;
type LRESULT = isize;
type HINSTANCE = isize;
type HGLOBAL = isize;
type HBRUSH = isize;
type LPCWSTR = *const u16;

const WM_DESTROY: u32 = 2;
const WM_CLIPBOARDUPDATE: u32 = 0x031D;
const WM_POWERBROADCAST: u32 = 0x218;
const PBT_APMRESUMESUSPEND: u32 = 7;
const PBT_APMRESUMEAUTOMATIC: u32 = 18;
const CF_UNICODETEXT: u32 = 13;
const CS_HREDRAW: u32 = 2;
const CS_VREDRAW: u32 = 1;
const WM_CREATE: u32 = 0x0001;
const WM_TIMER: u32 = 0x0113;
const ID_TIMER_FLUSH: usize = 1; // 异步刷盘定时器 ID

#[repr(C)]
struct WNDCLASSW {
    style: u32,
    lpfnWndProc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
    cbClsExtra: i32,
    cbWndExtra: i32,
    hInstance: HINSTANCE,
    hIcon: isize,
    hCursor: isize,
    hbrBackground: HBRUSH,
    lpszMenuName: LPCWSTR,
    lpszClassName: LPCWSTR,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct MSG {
    hwnd: HWND,
    message: u32,
    wParam: WPARAM,
    lParam: LPARAM,
    time: u32,
    pt: (i32, i32),
}

#[repr(C)]
#[derive(Copy, Clone)]
struct SYSTEMTIME {
    wYear: u16,
    wMonth: u16,
    wDayOfWeek: u16,
    wDay: u16,
    wHour: u16,
    wMinute: u16,
    wSecond: u16,
    wMilliseconds: u16,
}

static LAST_CLIP: Mutex<Option<String>> = Mutex::new(None);
static PROCESSING: AtomicBool = AtomicBool::new(false);
/// 待写入文件的队列: (日期字符串, 时间字符串, 剪切板内容)
/// 每条记录复制时的日期,即使刷盘跨日也能写入正确日期对应的文件
static PENDING: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());
/// 刷盘防重入标志(TIMER 触发时用;WM_DESTROY 退出时忽略,强制 flush)
static FLUSHING: AtomicBool = AtomicBool::new(false);

#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(wnd: *const WNDCLASSW) -> u16;
    fn CreateWindowExW(
        ex_style: u32, cls: LPCWSTR, title: LPCWSTR, style: u32,
        x: i32, y: i32, w: i32, h: i32,
        parent: HWND, menu: isize, inst: HINSTANCE, param: *mut c_void,
    ) -> HWND;
    fn DefWindowProcW(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
    fn DispatchMessageW(msg: *const MSG) -> LRESULT;
    fn GetMessageW(msg: *mut MSG, hwnd: HWND, min: u32, max: u32) -> i32;
    fn PostQuitMessage(code: i32);
    fn TranslateMessage(msg: *const MSG) -> i32;
    fn GetModuleHandleW(name: LPCWSTR) -> HINSTANCE;
    fn AddClipboardFormatListener(hwnd: HWND) -> i32;
    fn RemoveClipboardFormatListener(hwnd: HWND) -> i32;
    fn SetTimer(hwnd: HWND, id_event: usize, elapse: u32, timerproc: *const c_void) -> usize;
    fn KillTimer(hwnd: HWND, id_event: usize) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetLocalTime(st: *mut SYSTEMTIME);
    fn GlobalLock(hmem: HGLOBAL) -> *mut c_void;
    fn GlobalUnlock(hmem: HGLOBAL) -> i32;
}

#[link(name = "user32")]
extern "system" {
    fn OpenClipboard(hwnd: HWND) -> i32;
    fn CloseClipboard() -> i32;
    fn IsClipboardFormatAvailable(fmt: u32) -> i32;
    fn GetClipboardData(fmt: u32) -> HGLOBAL;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn get_save_dir() -> PathBuf {
    std::env::current_exe()
        .map(|p| p.parent().unwrap_or_else(|| std::path::Path::new(".")).to_path_buf())
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn get_today_string() -> String {
    unsafe {
        let mut st = MaybeUninit::<SYSTEMTIME>::uninit();
        GetLocalTime(st.as_mut_ptr());
        let st = st.assume_init();
        format!("{:04}{:02}{:02}", st.wYear, st.wMonth, st.wDay)
    }
}

fn get_time_string() -> String {
    unsafe {
        let mut st = MaybeUninit::<SYSTEMTIME>::uninit();
        GetLocalTime(st.as_mut_ptr());
        let st = st.assume_init();
        format!(
            "{:04}\u{5E74}{:02}\u{6708}{:02}\u{65E5} {:02}\u{65F6}{:02}\u{5206}{:02}\u{79D2}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
        )
    }
}

fn save_to_file(path: &PathBuf, tm: &str, text: &str) -> std::io::Result<()> {
    let sep = "\u{2550}".repeat(22);
    let old = fs::read_to_string(path).unwrap_or_default();
    let new_block = format!("{}\r\n{}\r\n\r\n{}\r\n\r\n", tm, text, sep);
    let content = format!("{}{}", new_block, old);
    fs::write(path, content.as_bytes())
}

fn process_clipboard() {
    if PROCESSING.load(Ordering::Relaxed) {
        return;
    }
    PROCESSING.store(true, Ordering::Relaxed);

    let _ = (|| -> std::result::Result<(), String> {
        unsafe {
            // 多监听器共存:重试 10 次,每次睡 1ms 让出 CPU 给其他进程
            // ClipboardAutoWrite 优先级高(要写文件),必须保证拿到锁
            let mut opened = false;
            let mut retries = 0u32;
            while retries < 10 {
                if OpenClipboard(0) != 0 {
                    opened = true;
                    break;
                }
                retries += 1;
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            if !opened { return Ok(()); }

            if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 { CloseClipboard(); return Ok(()); }

            let handle = GetClipboardData(CF_UNICODETEXT);
            if handle == 0 { CloseClipboard(); return Ok(()); }

            let ptr = GlobalLock(handle);
            if ptr.is_null() { CloseClipboard(); return Ok(()); }

            let mut len: usize = 0;
            while *(ptr as *const u16).add(len) != 0 { len += 1; }
            let slice = std::slice::from_raw_parts(ptr as *const u16, len);
            let text = String::from_utf16_lossy(slice);
            GlobalUnlock(handle);
            CloseClipboard();

            {
                let mut last = LAST_CLIP.lock().unwrap();
                if let Some(ref prev) = *last {
                    if prev == &text { return Ok(()); }
                }
                *last = Some(text.clone());
            }

            // 关键改动:不再立刻写文件!push 到内存队列,由 WM_TIMER 异步刷盘
            // 日期和时间都在复制时记录,避免 500ms 刷盘延迟导致跨天写错文件
            let td = get_today_string();
            let tm = get_time_string();
            {
                let mut queue = PENDING.lock().unwrap();
                queue.push((td, tm, text));
            }
        }
        Ok(())
    })();

    PROCESSING.store(false, Ordering::Relaxed);
}

/// 将 PENDING 队列中的内容按复制日期分组后批量写入文件
/// ignore_flushing: 是否忽略 FLUSHING 防重入(WM_DESTROY 退出时传 true,确保最后一批不丢)
fn flush_pending(ignore_flushing: bool) {
    if !ignore_flushing {
        // 正常 TIMER 触发:防重入,上一次还没写完就跳过(500ms 后下一次再刷)
        if FLUSHING.swap(true, Ordering::Relaxed) {
            return;
        }
    } else {
        // 退出时:如果刚好在 flush,等它完成(最多等 500ms),避免重复写
        let mut waited = 0u32;
        while FLUSHING.load(Ordering::Relaxed) && waited < 50 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            waited += 1;
        }
        FLUSHING.store(true, Ordering::Relaxed);
    }

    let items: Vec<(String, String, String)> = {
        let mut queue = match PENDING.lock() {
            Ok(g) => g,
            Err(_) => { FLUSHING.store(false, Ordering::Relaxed); return; }
        };
        if queue.is_empty() {
            drop(queue);
            FLUSHING.store(false, Ordering::Relaxed);
            return;
        }
        // 一次性 drain 所有待写项,尽快释放 Mutex 避免阻塞后续 push
        queue.drain(..).collect()
    };

    // 按复制日期分组,分别写入对应日期的 ClipboardYYYYMMDD.txt
    // 解决 23:59 复制、00:00 才刷盘导致昨天内容写进今天文件的问题
    // 以及 500ms 内批量复制跨 0 点混合队列的问题
    use std::collections::BTreeMap;
    let mut by_day: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (td, tm, text) in items {
        by_day.entry(td).or_default().push((tm, text));
    }

    let save_dir = get_save_dir();
    let dir: PathBuf = save_dir.canonicalize().unwrap_or(save_dir.clone());
    {
        let _ = fs::create_dir_all(&dir);
        for (td, entries) in by_day {
            let mut path = dir.clone();
            path.push(format!("Clipboard{}.txt", td));
            for (tm, text) in entries {
                let _ = save_to_file(&path, &tm, &text);
            }
        }
    }

    FLUSHING.store(false, Ordering::Relaxed);
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            // 500ms 定时器:异步刷盘 PENDING 队列
            // 这样 WM_CLIPBOARDUPDATE 只负责读剪切板+入队,最快速度释放锁
            let _ = SetTimer(hwnd, ID_TIMER_FLUSH, 500, std::ptr::null());
            0
        }
        WM_TIMER => {
            if wparam == ID_TIMER_FLUSH {
                flush_pending(false);
                return 0;
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_CLIPBOARDUPDATE => {
            process_clipboard();
            0
        }
        WM_POWERBROADCAST => {
            let event = wparam as u32;
            if event == PBT_APMRESUMESUSPEND || event == PBT_APMRESUMEAUTOMATIC {
                AddClipboardFormatListener(hwnd);
            }
            1
        }
        WM_DESTROY => {
            // ignore_flushing=true:强制 flush 残留内容,最多等 500ms;保证退出前不丢数据
            flush_pending(true);
            let _ = KillTimer(hwnd, ID_TIMER_FLUSH);
            RemoveClipboardFormatListener(hwnd);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Check if running with admin privileges
fn is_elevated() -> bool {
    let output = Command::new("net").arg("session")
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).contains("\\\\"),
        Err(_) => false,
    }
}

/// Write registry auto-start entry (no admin needed)
fn setup_registry(exe_str: &str) {
    let mut cmd = Command::new("reg");
    cmd.arg("add");
    cmd.arg(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run");
    cmd.arg("/v");
    cmd.arg("ClipboardAutoWrite");
    cmd.arg("/t");
    cmd.arg("REG_SZ");
    cmd.arg("/d");
    cmd.arg(exe_str);
    cmd.arg("/f");
    cmd.creation_flags(CREATE_NO_WINDOW);
    let _ = cmd.output();
}

/// Create scheduled task with restart-on-failure (requires admin)
fn setup_task(exe_str: &str) {
    // Delete old task first
    let mut del = Command::new("schtasks");
    del.arg("/delete");
    del.arg("/tn");
    del.arg("ClipboardAutoWrite_Wake");
    del.arg("/f");
    del.creation_flags(CREATE_NO_WINDOW);
    let _ = del.output();

    // Build XML with restart-on-failure settings
    let xml_content = format!(
r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Clipboard Auto Write - restart on failure</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>true</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>3</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{escaped}</Command>
    </Exec>
  </Actions>
</Task>"#,
        escaped = exe_str.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    );

    let tmp_dir = std::env::temp_dir();
    let xml_path = tmp_dir.join("clipboard_task.xml");
    let _ = fs::write(&xml_path, xml_content);

    let mut create = Command::new("schtasks");
    create.arg("/create");
    create.arg("/tn");
    create.arg("ClipboardAutoWrite_Wake");
    create.arg("/xml");
    create.arg(xml_path.to_string_lossy().as_ref());
    create.arg("/f");
    create.creation_flags(CREATE_NO_WINDOW);
    let _ = create.output();

    let _ = fs::remove_file(&xml_path);
}

fn setup_autostart() {
    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };
    let exe_str = exe_path.to_string_lossy().to_string();

    // Always write registry (no admin needed)
    setup_registry(&exe_str);

    // Only create scheduled task if running as admin
    if is_elevated() {
        setup_task(&exe_str);
    }
}

fn main() {
    unsafe {
        let inst = GetModuleHandleW(std::ptr::null());
        let class_name = wide("ClipboardAutoWriteClass");
        let window_name = wide("ClipboardAutoWrite");

        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: wnd_proc,
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: inst,
            hIcon: 0,
            hCursor: 0,
            hbrBackground: 0,
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };

        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            0, class_name.as_ptr(), window_name.as_ptr(), 0,
            0, 0, 0, 0, 0, 0, inst, std::ptr::null_mut(),
        );

        if hwnd == 0 { return; }

        AddClipboardFormatListener(hwnd);
        setup_autostart();

        let mut msg = MaybeUninit::<MSG>::uninit();
        while GetMessageW(msg.as_mut_ptr(), 0, 0, 0) != 0 {
            TranslateMessage(&*msg.as_ptr());
            DispatchMessageW(msg.as_ptr());
        }
    }
}
