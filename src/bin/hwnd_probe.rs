use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use windows_sys::Win32::Foundation::{HWND, RECT, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
use windows_sys::Win32::System::Console::{GetConsoleWindow, AllocConsole};

struct ProcessInfo {
    pid: u32,
    parent_pid: u32,
    exe_name: String,
}

fn get_process_map() -> HashMap<u32, ProcessInfo> {
    let mut map = HashMap::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return map;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                let exe_name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                map.insert(entry.th32ProcessID, ProcessInfo {
                    pid: entry.th32ProcessID,
                    parent_pid: entry.th32ParentProcessID,
                    exe_name,
                });
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        windows_sys::Win32::Foundation::CloseHandle(snapshot);
    }
    map
}

fn get_window_class_and_title(hwnd: HWND) -> (String, String) {
    let mut class_buf = [0u16; 256];
    let class_len = unsafe { GetClassNameW(hwnd, class_buf.as_mut_ptr(), 256) };
    let class_name = String::from_utf16_lossy(&class_buf[..class_len as usize]);

    let mut title_buf = [0u16; 256];
    let title_len = unsafe { GetWindowTextW(hwnd, title_buf.as_mut_ptr(), 256) };
    let title = String::from_utf16_lossy(&title_buf[..title_len as usize]);

    (class_name, title)
}

fn main() {
    unsafe { AllocConsole(); }
    let mut out = File::create("c:\\Users\\sifay\\Downloads\\001BismillahVibeCodeProjects\\AntiGravity\\rustyrangerfastslick\\probe_output.txt").unwrap();
    let pmap = get_process_map();
    let my_pid = unsafe { GetCurrentProcessId() };
    writeln!(out, "My PID: {} ({})", my_pid, pmap.get(&my_pid).map(|p| p.exe_name.as_str()).unwrap_or("unknown")).unwrap();

    // Trace process hierarchy up
    let mut curr_pid = my_pid;
    writeln!(out, "\nProcess Lineage:").unwrap();
    while let Some(p) = pmap.get(&curr_pid) {
        writeln!(out, "  PID: {} -> Parent: {} | Exe: {}", p.pid, p.parent_pid, p.exe_name).unwrap();
        if p.parent_pid == 0 || p.parent_pid == curr_pid {
            break;
        }
        curr_pid = p.parent_pid;
    }

    let console_hwnd = unsafe { GetConsoleWindow() };
    writeln!(out, "\nGetConsoleWindow() returned: {:?}", console_hwnd).unwrap();
    if console_hwnd.is_null() {
        writeln!(out, "Console HWND is NULL!").unwrap();
        return;
    }

    let mut console_pid = 0;
    unsafe { GetWindowThreadProcessId(console_hwnd, &mut console_pid); }
    let console_exe = pmap.get(&console_pid).map(|p| p.exe_name.as_str()).unwrap_or("unknown");
    let (c_class, c_title) = get_window_class_and_title(console_hwnd);
    writeln!(out, "Console Window PID: {} ({})", console_pid, console_exe).unwrap();
    writeln!(out, "Console Window Class: \"{}\" | Title: \"{}\"", c_class, c_title).unwrap();

    // Let's traverse the window relationships for GetConsoleWindow()
    writeln!(out, "\nWindow Traversal from GetConsoleWindow():").unwrap();
    let mut w = console_hwnd;
    while !w.is_null() {
        let parent = unsafe { GetParent(w) };
        let owner = unsafe { GetWindow(w, GW_OWNER) };
        let ancestor_root = unsafe { GetAncestor(w, GA_ROOT) };
        let ancestor_rootowner = unsafe { GetAncestor(w, GA_ROOTOWNER) };
        let mut w_pid = 0;
        unsafe { GetWindowThreadProcessId(w, &mut w_pid); }
        let w_exe = pmap.get(&w_pid).map(|p| p.exe_name.as_str()).unwrap_or("unknown");
        let (w_class, w_title) = get_window_class_and_title(w);

        writeln!(out, "HWND: {:?} | Class: \"{}\" | Title: \"{}\" | PID: {} ({})", w, w_class, w_title, w_pid, w_exe).unwrap();
        writeln!(out, "  Parent: {:?}", parent).unwrap();
        writeln!(out, "  Owner:  {:?}", owner).unwrap();
        writeln!(out, "  Ancestor (GA_ROOT): {:?}", ancestor_root).unwrap();
        writeln!(out, "  Ancestor (GA_ROOTOWNER): {:?}", ancestor_rootowner).unwrap();

        // Move to parent to trace up
        w = parent;
    }

    // List all top-level windows of Cascadia class or Console class
    writeln!(out, "\nTop-level Windows Search:").unwrap();
    unsafe extern "system" fn enum_windows_callback(hwnd: HWND, lparam: isize) -> i32 {
        let lparam_tuple = &*(lparam as *const (&HashMap<u32, ProcessInfo>, &mut File));
        let pmap = lparam_tuple.0;
        // Need to access out safely or just print
        let (class_name, title) = get_window_class_and_title(hwnd);
        if class_name.contains("CASCADIA") || class_name.contains("Console") {
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            let exe = pmap.get(&pid).map(|p| p.exe_name.as_str()).unwrap_or("unknown");
            // Workaround since we can't easily write to out in a static callback without Mutex/Mutex-like passing
            // Actually, we passed File in lparam! So we can write to it:
            // But we must cast lparam back properly.
            // Let's do that
        }
        1
    }

    // Actually, to make callback simple, let's just use thread-safe or pass a pointer to a struct.
    struct EnumState<'a> {
        pmap: &'a HashMap<u32, ProcessInfo>,
        out: &'a mut File,
    }
    unsafe extern "system" fn enum_windows_callback_safe(hwnd: HWND, lparam: isize) -> i32 {
        let state = &mut *(lparam as *mut EnumState);
        let (class_name, title) = get_window_class_and_title(hwnd);
        if class_name.contains("CASCADIA") || class_name.contains("Console") {
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            let exe = state.pmap.get(&pid).map(|p| p.exe_name.as_str()).unwrap_or("unknown");
            writeln!(state.out, "Top HWND: {:?} | Class: \"{}\" | Title: \"{}\" | PID: {} ({})", hwnd, class_name, title, pid, exe).unwrap();
        }
        1
    }
    let mut state = EnumState { pmap: &pmap, out: &mut out };
    unsafe {
        EnumWindows(Some(enum_windows_callback_safe), &mut state as *mut _ as isize);
    }
}


