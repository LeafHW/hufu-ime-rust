//! A dedicated, message-pumped STA for slow cross-process UI Automation work.
//! Only owned input/results cross the boundary; COM interfaces stay on this thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_FAILED};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::{CreateEventW, SetEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MsgWaitForMultipleObjectsEx, PeekMessageW, TranslateMessage, MSG,
    MWMO_INPUTAVAILABLE, PM_NOREMOVE, PM_REMOVE, QS_ALLINPUT, WM_QUIT,
};

type Job = Box<dyn FnOnce() + Send + 'static>;
const QUEUED_JOBS: usize = 8;

struct WakeEvent(HANDLE);
// Windows event handles support cross-thread SetEvent/wait. Arc keeps this
// private handle alive for both the submitter and the detached worker.
unsafe impl Send for WakeEvent {}
unsafe impl Sync for WakeEvent {}
impl Drop for WakeEvent {
    fn drop(&mut self) {
        unsafe { let _ = CloseHandle(self.0); }
    }
}

struct Task {
    job: Job,
    cancelled: Arc<AtomicBool>,
}

pub struct StaWorker {
    sender: Option<mpsc::SyncSender<Task>>,
    wake: Arc<WakeEvent>,
    stopping: Arc<AtomicBool>,
}

impl StaWorker {
    pub fn new() -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel::<Task>(QUEUED_JOBS);
        let (ready, started) = mpsc::sync_channel(1);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = stopping.clone();
        let wake = Arc::new(WakeEvent(unsafe { CreateEventW(None, false, false, None) }
            .map_err(|error| format!("无法创建 QQ 后台唤醒事件：{error}"))?));
        let worker_wake = wake.clone();
        std::thread::Builder::new()
            .name("trainer-uia-sta".into())
            .spawn(move || unsafe {
                let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                if initialized.is_err() {
                    let _ = ready.send(Err(format!("无法初始化 QQ 后台 STA：{initialized}")));
                    return;
                }
                struct Apartment;
                impl Drop for Apartment {
                    fn drop(&mut self) {
                        unsafe { CoUninitialize() };
                    }
                }
                let _apartment = Apartment;
                // Establish a native queue before creating/using STA COM objects.
                let mut msg = MSG::default();
                let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
                if ready.send(Ok(())).is_err() {
                    return;
                }
                'messages: while !worker_stopping.load(Ordering::Acquire) {
                    // Pump a bounded batch before each job so neither a burst
                    // of tasks nor a busy COM/window queue starves the other.
                    for _ in 0..32 {
                        if worker_stopping.load(Ordering::Acquire) {
                            break 'messages;
                        }
                        if !PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                            break;
                        }
                        if msg.message == WM_QUIT {
                            break 'messages;
                        }
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    let task = loop {
                        if worker_stopping.load(Ordering::Acquire) {
                            break 'messages;
                        }
                        match receiver.try_recv() {
                            Ok(task) if task.cancelled.load(Ordering::Acquire) => continue,
                            Ok(task) => break Some(task),
                            Err(mpsc::TryRecvError::Empty) => break None,
                            Err(mpsc::TryRecvError::Disconnected) => break 'messages,
                        }
                    };
                    if let Some(task) = task {
                        (task.job)();
                        continue;
                    }
                    // The queue is the task source of truth. A private event
                    // cannot be swallowed by UIA/COM's nested modal message pump
                    // (unlike hwnd=null PostThreadMessage notifications). Check
                    // the queue again after every job, including coalesced wakes.
                    if MsgWaitForMultipleObjectsEx(Some(&[worker_wake.0]), u32::MAX,
                        QS_ALLINPUT, MWMO_INPUTAVAILABLE) == WAIT_FAILED
                    {
                        break;
                    }
                }
                // Closing the receiver drops any remaining jobs without running
                // them after the trainer has exited.
                drop(receiver);
            })
            .map_err(|e| format!("无法启动 QQ 后台线程：{e}"))?;
        match started.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(Self { sender: Some(sender), wake, stopping }),
            Ok(Err(error)) => Err(error),
            Err(error) => {
                stopping.store(true, Ordering::Release);
                unsafe { let _ = SetEvent(wake.0); }
                Err(format!("QQ 后台线程未能就绪：{error}"))
            }
        }
    }

    /// Enqueue without waiting for QQ/UIA or for another operation to finish.
    pub fn submit(&self, job: Job) -> Result<(), String> {
        if self.stopping.load(Ordering::Acquire) {
            return Err("QQ 后台线程已停止".into());
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let task = Task { job, cancelled: cancelled.clone() };
        self.sender.as_ref().ok_or("QQ 后台线程已停止")?
            .try_send(task)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => "QQ 后台任务过多，请稍后重试".to_string(),
                mpsc::TrySendError::Disconnected(_) => "QQ 后台线程不可用，请重启跟打器".to_string(),
            })?;
        unsafe { SetEvent(self.wake.0) }
            .map_err(|error| {
                // If the queue cannot be signalled, a later wake must not execute
                // an operation that the caller was already told had failed.
                cancelled.store(true, Ordering::Release);
                format!("无法唤醒 QQ 后台线程：{error}")
            })
    }
}

impl Drop for StaWorker {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        self.sender.take();
        unsafe { let _ = SetEvent(self.wake.0); }
        // Intentionally detach: a third-party accessibility provider may hang.
        // Waiting for that call here would make closing the trainer hang too.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP};

    #[test]
    fn jobs_keep_one_sta_and_do_not_block_the_submitter() {
        use windows::Win32::System::Com::{
            CoGetApartmentType, APTTYPE, APTTYPEQUALIFIER, APTTYPE_MAINSTA, APTTYPE_STA,
        };
        let worker = StaWorker::new().unwrap();
        let (result, done) = mpsc::channel();
        for expected in 0..3 {
            let result = result.clone();
            worker.submit(Box::new(move || unsafe {
                let mut apartment = APTTYPE::default();
                let mut qualifier = APTTYPEQUALIFIER::default();
                CoGetApartmentType(&mut apartment, &mut qualifier).unwrap();
                result.send((expected, GetCurrentThreadId(), apartment)).unwrap();
            })).unwrap();
        }
        let first = done.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(first.0, 0);
        // The first STA created by a test process is reported as MAINSTA.
        assert!(first.2 == APTTYPE_STA || first.2 == APTTYPE_MAINSTA);
        for expected in 1..3 {
            let value = done.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(value, (expected, first.1, first.2));
        }
    }

    #[test]
    fn idle_worker_dispatches_native_timer_callbacks() {
        use std::cell::RefCell;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
        thread_local! {
            static COMPLETED: RefCell<Option<mpsc::Sender<()>>> = const { RefCell::new(None) };
        }
        unsafe extern "system" fn tick(_: HWND, _: u32, timer: usize, _: u32) {
            let _ = KillTimer(None, timer);
            COMPLETED.with(|slot| {
                if let Some(done) = slot.borrow_mut().take() {
                    let _ = done.send(());
                }
            });
        }
        let worker = StaWorker::new().unwrap();
        let (result, done) = mpsc::channel();
        worker.submit(Box::new(move || unsafe {
            COMPLETED.with(|slot| *slot.borrow_mut() = Some(result));
            assert_ne!(SetTimer(None, 0, 15, Some(tick)), 0);
        })).unwrap();
        // No second job wakes the worker; DispatchMessage must service WM_TIMER.
        done.recv_timeout(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn cancelled_job_without_a_wake_does_not_strand_the_next_job() {
        let worker = StaWorker::new().unwrap();
        let (result, done) = mpsc::channel();
        let cancelled_result = result.clone();
        // Reproduce a submit whose queue insertion succeeded but native wake
        // failed: its cancellation flag is set without signalling the event.
        worker.sender.as_ref().unwrap().try_send(Task {
            job: Box::new(move || { let _ = cancelled_result.send(0); }),
            cancelled: Arc::new(AtomicBool::new(true)),
        }).unwrap_or_else(|_| panic!("fresh worker queue must accept one task"));
        worker.submit(Box::new(move || { let _ = result.send(1); })).unwrap();
        assert_eq!(done.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
    }

    #[test]
    fn a_nested_modal_pump_cannot_steal_queued_job_wakes() {
        let worker = StaWorker::new().unwrap();
        let (entered, running) = mpsc::channel();
        let (continue_pump, begin_pump) = mpsc::channel();
        let (drained, observed) = mpsc::channel();
        worker.submit(Box::new(move || unsafe {
            entered.send(GetCurrentThreadId()).unwrap();
            begin_pump.recv().unwrap();
            // Reproduce a nested COM/provider modal loop: it can remove thread
            // messages while DispatchMessage cannot route their hwnd=null.
            let mut msg = MSG::default();
            let mut count = 0;
            while PeekMessageW(&mut msg, None, 0, 0,
                windows::Win32::UI::WindowsAndMessaging::PM_REMOVE).as_bool()
            {
                count += 1;
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            drained.send(count).unwrap();
        })).unwrap();
        let id = running.recv_timeout(Duration::from_secs(2)).unwrap();
        unsafe { PostThreadMessageW(id, WM_APP + 92, WPARAM(0), LPARAM(0)).unwrap(); }
        let (result, done) = mpsc::channel();
        for expected in 0..2 {
            let result = result.clone();
            worker.submit(Box::new(move || { let _ = result.send(expected); })).unwrap();
        }
        continue_pump.send(()).unwrap();
        assert!(observed.recv_timeout(Duration::from_secs(2)).unwrap() >= 1);
        for expected in 0..2 {
            assert_eq!(done.recv_timeout(Duration::from_secs(2)).unwrap(), expected);
        }
    }

    #[test]
    fn a_busy_provider_has_bounded_queue_and_never_delays_drop() {
        let worker = StaWorker::new().unwrap();
        let (entered, running) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        worker.submit(Box::new(move || {
            entered.send(()).unwrap();
            let _ = blocked.recv();
        })).unwrap();
        running.recv_timeout(Duration::from_secs(2)).unwrap();
        for _ in 0..QUEUED_JOBS {
            worker.submit(Box::new(|| {})).unwrap();
        }
        assert!(worker.submit(Box::new(|| {})).is_err());
        let started = std::time::Instant::now();
        drop(worker);
        let elapsed = started.elapsed();
        release.send(()).unwrap();
        assert!(elapsed < Duration::from_millis(100), "drop waited for an inaccessible provider");
    }
}
