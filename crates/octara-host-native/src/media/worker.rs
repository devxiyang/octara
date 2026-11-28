use octara::runtime::task::{Receiver, channel};
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex};

type Job = Box<dyn FnOnce() + Send>;
const WORKERS: usize = 4;
const QUEUE_LIMIT: usize = 32;

#[derive(Default)]
struct Queue {
    foreground: VecDeque<Job>,
    maintenance: VecDeque<Job>,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}

pub(super) struct Pool(Arc<Shared>);

impl Pool {
    pub(super) fn new(setup: fn()) -> Result<Self, Arc<str>> {
        let pool = Self(Arc::new(Shared::default()));
        for index in 0..WORKERS {
            let shared = pool.0.clone();
            std::thread::Builder::new()
                .name(format!("octara-media-{index}"))
                .spawn(move || {
                    setup();
                    shared.run(index == 0);
                })
                .map_err(|_| Arc::from("Image worker could not start"))?;
        }
        Ok(pool)
    }

    pub(super) fn submit<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T, Arc<str>> + Send + 'static,
    ) -> Result<Receiver<Result<T, Arc<str>>>, Arc<str>> {
        let (send, receive) = channel();
        self.enqueue(
            false,
            Box::new(move || {
                let outcome = catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|_| Err(Arc::from("Image worker stopped")));
                let _ = send.send(outcome);
            }),
        )?;
        Ok(receive)
    }

    pub(super) fn maintain(&self, work: impl FnOnce() + Send + 'static) {
        let _ = self.enqueue(true, Box::new(work));
    }

    fn enqueue(&self, maintenance: bool, job: Job) -> Result<(), Arc<str>> {
        {
            let mut queue = self.0.queue.lock().expect("media queue");
            if queue.closed || queue.foreground.len() + queue.maintenance.len() >= QUEUE_LIMIT {
                return Err(Arc::from("Image worker queue is full"));
            }
            if maintenance {
                queue.maintenance.push_back(job);
            } else {
                queue.foreground.push_back(job);
            }
        }
        // Only one worker handles revalidation. Keep the others free for visible requests.
        self.0.ready.notify_all();
        Ok(())
    }
}

impl Shared {
    fn run(&self, allow_maintenance: bool) {
        loop {
            let job = {
                let mut queue = self.queue.lock().expect("media queue");
                loop {
                    if queue.closed {
                        return;
                    }
                    if let Some(job) =
                        queue.foreground.pop_front().or_else(|| if allow_maintenance { queue.maintenance.pop_front() } else { None })
                    {
                        break job;
                    }
                    queue = self.ready.wait(queue).expect("media queue");
                }
            };
            let _ = catch_unwind(AssertUnwindSafe(job));
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        let jobs = {
            let mut queue = self.0.queue.lock().expect("media queue");
            queue.closed = true;
            (std::mem::take(&mut queue.foreground), std::mem::take(&mut queue.maintenance))
        };
        self.0.ready.notify_all();
        drop(jobs);
    }
}
