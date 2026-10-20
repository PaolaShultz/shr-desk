//! Optional GP-SPECTRUM:1 observer. No audio analysis or invented live samples.
use crate::{frontend::Config, local_audio::Transport, metering::Identity};
use serde::Deserialize;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub contract: String,
    pub version: u32,
    pub supported: bool,
    pub show_id: String,
    pub epoch: u64,
    pub map: u64,
    pub query: u64,
    pub input: String,
    pub sample_rate: u32,
    pub tap: String,
    pub units: String,
    pub first_frame: u64,
    pub end_frame: u64,
    pub acquisition_age_ms: u64,
    pub valid: bool,
    pub bins: Vec<i32>,
}
#[derive(Clone, Debug)]
pub struct View {
    pub bins: Vec<i32>,
    pub peaks: Vec<i32>,
    pub end: u64,
    expires: Instant,
}
impl View {
    pub fn fresh(&self) -> bool {
        Instant::now() < self.expires
    }
}
pub struct Cache {
    previous: Option<View>,
    hold: [u64; 64],
}
impl Default for Cache {
    fn default() -> Self {
        Self {
            previous: None,
            hold: [0; 64],
        }
    }
}
impl Cache {
    pub fn accept(
        &mut self,
        s: Snapshot,
        id: &Identity,
        input: &str,
        query: u64,
        rtt: Duration,
    ) -> Result<(), String> {
        if s.contract != "GP-SPECTRUM"
            || s.version != 1
            || !s.supported
            || s.query != query
            || s.show_id != id.show
            || s.epoch != id.epoch
            || s.map != id.map
            || s.input != input
            || s.sample_rate != id.rate
            || s.tap != "raw-pre-processing"
            || s.units != "millidBFS-peak-Hann-max"
        {
            return Err("spectrum identity".into());
        }
        if !s.valid {
            self.previous = None;
            return Ok(());
        }
        if s.bins.len() != 64
            || s.bins.iter().any(|v| !(-96000..=24000).contains(v))
            || s.end_frame.checked_sub(s.first_frame) != Some(4096)
            || s.acquisition_age_ms > 350
            || s.end_frame > u64::MAX - id.rate as u64
        {
            return Err("spectrum values".into());
        }
        if self.previous.as_ref().is_some_and(|p| s.end_frame <= p.end) {
            return Ok(());
        }
        let mut peaks = s.bins.clone();
        if let Some(p) = &self.previous {
            for (i, peak) in peaks.iter_mut().enumerate() {
                if s.bins[i] >= p.peaks[i] {
                    self.hold[i] = s.end_frame + id.rate as u64 / 2;
                } else if s.end_frame <= self.hold[i] {
                    *peak = p.peaks[i];
                } else {
                    *peak = s.bins[i].max(
                        p.peaks[i]
                            - (s.end_frame
                                .saturating_sub(p.end.max(self.hold[i]))
                                .saturating_mul(24000)
                                / id.rate as u64)
                                .min(120000) as i32,
                    );
                }
            }
        } else {
            self.hold.fill(s.end_frame + id.rate as u64 / 2);
        }
        let remaining = Duration::from_millis(350 - s.acquisition_age_ms).saturating_sub(rtt);
        self.previous = Some(View {
            bins: s.bins,
            peaks,
            end: s.end_frame,
            expires: Instant::now() + remaining,
        });
        Ok(())
    }
    pub fn view(&self) -> Option<View> {
        self.previous.clone().filter(View::fresh)
    }
}
pub struct Worker {
    pub key: String,
    latest: Arc<Mutex<Option<View>>>,
    stop: Arc<AtomicBool>,
}
impl Worker {
    pub fn start(config: Config, id: Identity, input: String, key: String) -> Self {
        let latest = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (out, halt) = (latest.clone(), stop.clone());
        thread::spawn(move || {
            let mut connection = None;
            let mut query = 0u64;
            let mut cache = Cache::default();
            let mut supported = true;
            let mut discovered = false;
            while !halt.load(Ordering::Relaxed) {
                let began = Instant::now();
                let deadline = began + Duration::from_millis(90);
                let result = (|| -> Result<(), String> {
                    if !supported {
                        return Ok(());
                    }
                    if connection.is_none() {
                        connection = Some(Transport::connect_until(&config.endpoint, deadline)?);
                    }
                    query = query.checked_add(1).ok_or("spectrum query exhausted")?;
                    let request = serde_json::json!({"contract":"GP-SPECTRUM","version":1,"show_id":id.show,"epoch":id.epoch,"map":id.map,"query":query,"input":input});
                    let c = connection.as_mut().unwrap();
                    c.send_frame_until(
                        &serde_json::to_vec(&request).map_err(|e| e.to_string())?,
                        deadline,
                    )?;
                    let bytes = c.receive_until(deadline)?.ok_or("spectrum missing")?;
                    if bytes.len() > 4096 {
                        return Err("spectrum capacity".into());
                    }
                    let v = crate::provider::parse_document(&bytes)?;
                    if v["contract"] != "GP-SPECTRUM" {
                        supported = false;
                        return Err("unsupported".into());
                    }
                    let s: Snapshot = serde_json::from_value(v).map_err(|e| e.to_string())?;
                    discovered = s.supported;
                    cache.accept(s, &id, &input, query, began.elapsed())?;
                    *out.lock().unwrap() = cache.view();
                    Ok(())
                })();
                if result.is_err() {
                    if !discovered {
                        supported = false;
                    }
                    connection = None;
                    cache = Cache::default();
                    *out.lock().unwrap() = None;
                }
                thread::sleep(Duration::from_millis(100).saturating_sub(began.elapsed()));
            }
        });
        Self { key, latest, stop }
    }
    pub fn view(&self) -> Option<View> {
        self.latest.lock().unwrap().clone().filter(View::fresh)
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
