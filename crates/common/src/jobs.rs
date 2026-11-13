//! One background job at a time, with a log the web UI polls.

use anyhow::{bail, Result};
use serde::Serialize;
use std::sync::{Arc, Mutex};

#[derive(Clone, Serialize, Default)]
pub struct JobView {
    pub title: String,
    pub lines: Vec<String>,
    pub error: Option<String>,
    pub running: bool,
}

/// Handle given to a running job for logging progress.
#[derive(Clone)]
pub struct Job(Arc<Mutex<JobView>>);

impl Job {
    pub fn log(&self, msg: impl Into<String>) {
        let msg = msg.into();
        let mut v = self.0.lock().unwrap();
        crate::log(format!("[{}] {msg}", v.title));
        v.lines.push(msg);
    }
}

#[derive(Clone, Default)]
pub struct JobRunner {
    current: Arc<Mutex<Option<Job>>>,
}

impl JobRunner {
    pub fn busy(&self) -> bool {
        self.current.lock().unwrap().as_ref().is_some_and(|j| j.0.lock().unwrap().running)
    }

    pub fn start<F>(&self, title: &str, f: F) -> Result<JobView>
    where
        F: FnOnce(&Job) -> Result<()> + Send + 'static,
    {
        let job = {
            let mut cur = self.current.lock().unwrap();
            if let Some(j) = cur.as_ref() {
                let v = j.0.lock().unwrap();
                if v.running {
                    bail!("Busy: {}", v.title);
                }
            }
            let job = Job(Arc::new(Mutex::new(JobView {
                title: title.to_string(),
                running: true,
                ..Default::default()
            })));
            *cur = Some(job.clone());
            job
        };
        let view = job.0.lock().unwrap().clone();
        std::thread::spawn(move || {
            let result = f(&job);
            match result {
                Ok(()) => job.log("Done"),
                Err(e) => {
                    crate::log(format!("job failed: {e:#}"));
                    job.0.lock().unwrap().error = Some(format!("{e:#}"));
                }
            }
            job.0.lock().unwrap().running = false;
        });
        Ok(view)
    }

    pub fn view(&self) -> Option<JobView> {
        self.current.lock().unwrap().as_ref().map(|j| {
            let mut v = j.0.lock().unwrap().clone();
            let n = v.lines.len();
            if n > 30 {
                v.lines.drain(..n - 30);
            }
            v
        })
    }
}
