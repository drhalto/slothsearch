//! A pure-Rust client for Everything 1.4's IPC (the protocol the official SDK DLL speaks).
//!
//! A query is a WM_COPYDATA sent to Everything's taskbar-notification window carrying an
//! EVERYTHING_IPC_QUERY2 header and the search text. Everything answers with another
//! WM_COPYDATA to our reply window: an EVERYTHING_IPC_LIST2 header, one ITEM2 per result,
//! then each item's fields in request-flag order. Layouts and field order come from the
//! SDK's `everything_ipc.h` and `Everything.c` (`_Everything_GetRequestData`).
//!
//! A `Client` owns a message-only window and must be used on the thread that created it,
//! which has to pump messages while it waits; `query` does that itself.

use std::cell::{Cell, RefCell};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::INFINITE;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::util::{from_wide, wide};

const COPYDATA_QUERY2W: usize = 18;

const REQUEST_NAME: u32 = 0x1;
const REQUEST_PATH: u32 = 0x2;
const REQUEST_FULL_PATH: u32 = 0x4;
const REQUEST_EXTENSION: u32 = 0x8;
const REQUEST_SIZE: u32 = 0x10;
const REQUEST_DATE_CREATED: u32 = 0x20;
const REQUEST_DATE_MODIFIED: u32 = 0x40;

const ITEM_FOLDER: u32 = 0x1;
const ITEM_DRIVE: u32 = 0x2;

pub const SORT_NAME: u32 = 1;
pub const SORT_DATE_MODIFIED_DESC: u32 = 14;

#[derive(Clone, Debug)]
pub struct Hit {
    pub name: String,
    /// The containing folder (empty for drive roots).
    pub dir: String,
    pub folder: bool,
    pub size: Option<u64>,
    /// Unix seconds.
    pub modified: Option<u64>,
}

impl Hit {
    pub fn path(&self) -> String {
        if self.dir.is_empty() {
            self.name.clone()
        } else if self.dir.ends_with('\\') {
            format!("{}{}", self.dir, self.name)
        } else {
            format!("{}\\{}", self.dir, self.name)
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// Everything's window isn't there: the tray client isn't running.
    NotRunning,
    /// Everything didn't accept the query, or didn't answer in time.
    NoReply,
}

#[derive(Clone, Debug)]
pub struct Results {
    pub total: u32,
    pub hits: Vec<Hit>,
}

thread_local! {
    static WAITING_FOR: Cell<usize> = const { Cell::new(0) };
    static REPLY: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

unsafe extern "system" fn reply_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_COPYDATA {
        let cds = unsafe { &*(lp.0 as *const COPYDATASTRUCT) };
        if cds.dwData == WAITING_FOR.get() && !cds.lpData.is_null() {
            let bytes = unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) };
            REPLY.with(|r| *r.borrow_mut() = Some(bytes.to_vec()));
            return LRESULT(1);
        }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

pub struct Client {
    hwnd: HWND,
    next_id: usize,
}

impl Client {
    pub fn new() -> Option<Client> {
        unsafe {
            let hinst = GetModuleHandleW(None).ok()?;
            let class = w!("SlothSearchEverythingReply");
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(reply_proc),
                hInstance: hinst.into(),
                lpszClassName: class,
                ..Default::default()
            };
            // Registering twice (second client thread) fails harmlessly.
            RegisterClassExW(&wc);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                w!(""),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(hinst.into()),
                None,
            )
            .ok()?;
            // Everything may run elevated; let its reply through UIPI.
            let _ = ChangeWindowMessageFilterEx(hwnd, WM_COPYDATA, MSGFLT_ALLOW, None);
            Some(Client { hwnd, next_id: 0x5052_0000 })
        }
    }

    pub fn query(&mut self, search: &str, max: u32, sort: u32, timeout: Duration) -> Result<Results, Error> {
        let everything = unsafe { FindWindowW(w!("EVERYTHING_TASKBAR_NOTIFICATION"), PCWSTR::null()) }
            .map_err(|_| Error::NotRunning)?;

        self.next_id += 1;
        let id = self.next_id;
        WAITING_FOR.set(id);
        REPLY.with(|r| *r.borrow_mut() = None);

        let request = REQUEST_NAME | REQUEST_PATH | REQUEST_SIZE | REQUEST_DATE_MODIFIED;
        let header: [u32; 7] = [
            self.hwnd.0 as usize as u32, // reply_hwnd: HWNDs fit in 32 bits, even on x64
            id as u32,                   // reply_copydata_message
            0,                           // search_flags
            0,                           // offset
            max,                         // max_results
            request,                     // request_flags
            sort,                        // sort_type
        ];
        let mut buf: Vec<u8> = header.iter().flat_map(|v| v.to_le_bytes()).collect();
        for unit in wide(search) {
            buf.extend_from_slice(&unit.to_le_bytes());
        }
        let cds = COPYDATASTRUCT {
            dwData: COPYDATA_QUERY2W,
            cbData: buf.len() as u32,
            lpData: buf.as_mut_ptr().cast(),
        };
        let accepted = unsafe {
            SendMessageW(
                everything,
                WM_COPYDATA,
                Some(WPARAM(self.hwnd.0 as usize)),
                Some(LPARAM(&cds as *const _ as isize)),
            )
        };
        if accepted.0 == 0 {
            return Err(Error::NoReply);
        }

        let deadline = Instant::now() + timeout;
        loop {
            if let Some(bytes) = REPLY.with(|r| r.borrow_mut().take()) {
                WAITING_FOR.set(0);
                return Ok(parse(&bytes));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                WAITING_FOR.set(0);
                return Err(Error::NoReply);
            }
            unsafe {
                MsgWaitForMultipleObjects(None, false, left.as_millis().min(INFINITE as u128 - 1) as u32, QS_ALLINPUT);
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    b.get(at..at + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()))
}

/// Reads a `DWORD len; WCHAR text[len + 1]` field, returning the text and the next offset.
fn string_at(b: &[u8], at: usize) -> Option<(String, usize)> {
    let len = u32_at(b, at)? as usize;
    let start = at + 4;
    let bytes = b.get(start..start + len * 2)?;
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Some((from_wide(&units), start + (len + 1) * 2))
}

fn parse(b: &[u8]) -> Results {
    let total = u32_at(b, 0).unwrap_or(0);
    let count = u32_at(b, 4).unwrap_or(0) as usize;
    let flags = u32_at(b, 12).unwrap_or(0);
    let mut hits = Vec::with_capacity(count);
    for i in 0..count {
        let item = 20 + i * 8;
        let (Some(item_flags), Some(data)) = (u32_at(b, item), u32_at(b, item + 4)) else { break };
        if let Some(hit) = parse_item(b, data as usize, flags, item_flags) {
            hits.push(hit);
        }
    }
    Results { total, hits }
}

fn parse_item(b: &[u8], mut p: usize, flags: u32, item_flags: u32) -> Option<Hit> {
    let mut name = String::new();
    let mut dir = String::new();
    let mut size = None;
    let mut modified = None;
    if flags & REQUEST_NAME != 0 {
        (name, p) = string_at(b, p)?;
    }
    if flags & REQUEST_PATH != 0 {
        (dir, p) = string_at(b, p)?;
    }
    if flags & REQUEST_FULL_PATH != 0 {
        (_, p) = string_at(b, p)?;
    }
    if flags & REQUEST_EXTENSION != 0 {
        (_, p) = string_at(b, p)?;
    }
    if flags & REQUEST_SIZE != 0 {
        let v = u64_at(b, p)?;
        // Folders report -1 unless folder sizes are indexed.
        size = (v != u64::MAX).then_some(v);
        p += 8;
    }
    if flags & REQUEST_DATE_CREATED != 0 {
        p += 8;
    }
    if flags & REQUEST_DATE_MODIFIED != 0 {
        let v = u64_at(b, p)?;
        modified = (v != 0 && v != u64::MAX).then(|| crate::util::filetime_to_unix(v));
    }
    let folder = item_flags & (ITEM_FOLDER | ITEM_DRIVE) != 0;
    Some(Hit { name, dir, folder, size, modified })
}

// ---------------------------------------------------------------------------------------
// Background worker. Everything answers one query at a time, and overlapping clients
// slow each other down badly (~60 ms alone, ~900 ms when two overlap), so every query
// SlothSearch makes goes through this one thread: searches first, upkeep when it's quiet.

#[derive(Clone, Debug)]
pub struct Search {
    pub text: String,
    pub max: u32,
    pub sort: u32,
}

pub struct Request {
    pub id: u64,
    pub searches: Vec<Search>,
}

pub struct Response {
    pub id: u64,
    pub results: Vec<Result<Results, Error>>,
    pub elapsed: Duration,
}

pub enum Job {
    Search(Request),
    /// How stale the index is: age of the newest change.
    Health,
    /// Rescan git repositories.
    Projects,
}

pub enum Reply {
    Search(Response),
    /// Seconds since the newest change Everything knows about; None if it isn't running.
    Health(Option<u64>),
    Projects(Vec<crate::projects::Project>),
}

/// Upkeep waits for this long without a search.
const QUIET: Duration = Duration::from_millis(250);

/// Spawns the query thread. `done` receives each reply (and should wake the UI).
pub fn spawn(done: impl Fn(Reply) + Send + 'static) -> Sender<Job> {
    let (tx, rx): (Sender<Job>, Receiver<Job>) = channel();
    std::thread::Builder::new()
        .name("everything".into())
        .spawn(move || {
            let Some(mut client) = Client::new() else { return };
            let mut upkeep: Vec<Job> = Vec::new();
            loop {
                let job = if upkeep.is_empty() {
                    match rx.recv() {
                        Ok(j) => j,
                        Err(_) => return,
                    }
                } else {
                    match rx.recv_timeout(QUIET) {
                        Ok(j) => j,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            let j = upkeep.remove(0);
                            done(run_upkeep(&mut client, j));
                            continue;
                        }
                        Err(_) => return,
                    }
                };
                let mut req = match job {
                    Job::Search(r) => r,
                    other => {
                        defer(&mut upkeep, other);
                        continue;
                    }
                };
                // Typing sends a burst; skip straight to the newest.
                while let Ok(j) = rx.try_recv() {
                    match j {
                        Job::Search(newer) => req = newer,
                        other => defer(&mut upkeep, other),
                    }
                }
                let start = Instant::now();
                let results = req
                    .searches
                    .iter()
                    .map(|s| {
                        let mut r = client.query(&s.text, s.max, s.sort, Duration::from_millis(1500));
                        if matches!(r, Err(Error::NoReply)) {
                            // Someone else's query (es.exe, Everything's window) got in the way.
                            r = client.query(&s.text, s.max, s.sort, Duration::from_millis(1500));
                        }
                        r
                    })
                    .collect();
                done(Reply::Search(Response { id: req.id, results, elapsed: start.elapsed() }));
            }
        })
        .expect("spawn everything thread");
    tx
}

fn defer(upkeep: &mut Vec<Job>, job: Job) {
    let same = |a: &Job| std::mem::discriminant(a) == std::mem::discriminant(&job);
    if !upkeep.iter().any(same) {
        upkeep.push(job);
    }
}

fn run_upkeep(client: &mut Client, job: Job) -> Reply {
    match job {
        Job::Projects => Reply::Projects(crate::projects::scan(client)),
        _ => {
            let age = client.query("", 25, SORT_DATE_MODIFIED_DESC, Duration::from_secs(2)).ok().map(|r| {
                let now = crate::util::now_secs();
                // Skip files stamped in the future.
                let newest = r.hits.iter().filter_map(|h| h.modified).find(|&m| m <= now + 60);
                newest.map_or(0, |m| now.saturating_sub(m))
            });
            Reply::Health(age)
        }
    }
}
