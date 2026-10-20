//! Optional GP-SPECTRUM:2 observer with explicit v1 fallback. No audio analysis or invented live samples.
use crate::{
    frontend::Config,
    local_audio::{AuthorityConnection, Transport},
    metering::Identity,
};
use serde::Deserialize;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub label: String,
    pub tap: String,
}
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
    #[serde(default)]
    pub fft_bins: Option<Vec<i32>>,
    #[serde(default)]
    pub sources: Option<Vec<Source>>,
}
#[derive(Clone, Debug)]
pub struct View {
    pub fft_bins: Option<Vec<i32>>,
    pub fft_peaks: Option<Vec<i32>>,
    pub rate: u32,
    pub epoch: u64,
    pub map: u64,
    pub source: String,
    pub tap: String,
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
#[derive(Default)]
pub struct Cache {
    previous: Option<View>,
    hold: Vec<u64>,
    identity: Option<(String, u64, u64, String, u32)>,
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
            || !matches!(s.version, 1 | 2)
            || !s.supported
            || s.query != query
            || s.show_id != id.show
            || s.epoch != id.epoch
            || s.map != id.map
            || s.input != input
            || s.sample_rate != id.rate
            || (s.version == 1 && s.tap != "raw-pre-processing")
            || (s.version == 2
                && !matches!(
                    s.tap.as_str(),
                    "raw-pre-processing" | "main-pre-pa" | "physical-output-post-patch"
                ))
            || s.units != "millidBFS-peak-Hann-max"
        {
            return Err("spectrum identity".into());
        }
        let identity = (
            id.show.clone(),
            id.epoch,
            id.map,
            input.to_string(),
            s.version,
        );
        if self.identity.as_ref() != Some(&identity) {
            self.previous = None;
            self.hold.clear();
            self.identity = Some(identity);
        }
        if s.version == 2 {
            let sources = s.sources.as_ref().ok_or("spectrum sources")?;
            if sources.is_empty()
                || sources.len() > 4096
                || !sources.iter().any(|x| x.id == input && x.tap == s.tap)
                || sources.iter().any(|x| {
                    x.id.is_empty()
                        || x.id.len() > 135
                        || x.label.len() > 2048
                        || !matches!(
                            x.tap.as_str(),
                            "raw-pre-processing" | "main-pre-pa" | "physical-output-post-patch"
                        )
                        || x.label.chars().any(char::is_control)
                })
                || sources
                    .iter()
                    .map(|x| &x.id)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != sources.len()
                || s.fft_bins.as_ref().is_none_or(|b| {
                    if s.valid {
                        b.len() != 2049 || b.iter().any(|v| !(-96000..=24000).contains(v))
                    } else {
                        !b.is_empty()
                    }
                })
            {
                return Err("spectrum v2 values/capabilities".into());
            }
        } else if s.fft_bins.is_some() || s.sources.is_some() {
            return Err("spectrum v1 extensions".into());
        }
        if !s.valid {
            if !s.bins.is_empty() {
                return Err("invalid spectrum has data".into());
            }
            self.hold.clear();
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
        let levels = s.fft_bins.as_ref().unwrap_or(&s.bins);
        let mut peaks = levels.clone();
        self.hold.resize(levels.len(), 0);
        if let Some(p) = &self.previous {
            let previous = p.fft_peaks.as_ref().unwrap_or(&p.peaks);
            for (i, peak) in peaks.iter_mut().enumerate() {
                if levels[i] >= previous[i] {
                    self.hold[i] = s.end_frame + id.rate as u64 / 2;
                } else if s.end_frame <= self.hold[i] {
                    *peak = previous[i];
                } else {
                    *peak = levels[i].max(
                        previous[i]
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
            fft_peaks: s.fft_bins.as_ref().map(|_| peaks.clone()),
            peaks: if s.fft_bins.is_some() {
                aggregate(&peaks, s.sample_rate, 64)
            } else {
                peaks
            },
            fft_bins: s.fft_bins,
            rate: s.sample_rate,
            epoch: s.epoch,
            map: s.map,
            source: s.input,
            tap: s.tap,
            bins: s.bins,
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
    sources: Arc<Mutex<Vec<Source>>>,
    stop: Arc<AtomicBool>,
    child: Option<thread::JoinHandle<()>>,
}
impl Worker {
    pub fn start(config: Config, id: Identity, input: String, key: String) -> Self {
        let latest = Arc::new(Mutex::new(None));
        let sources = Arc::new(Mutex::new(Vec::new()));
        let inventory = sources.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let (out, halt) = (latest.clone(), stop.clone());
        let child = thread::spawn(move || {
            let mut connection: Option<Box<dyn AuthorityConnection>> = None;
            let mut version = 2;
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
                        connection = Some(if let Some(remote) = &config.remote {
                            let c = crate::remote::Connection::connect_observer(remote, deadline)?;
                            if c.source_epoch != id.epoch {
                                return Err("spectrum remote epoch".into());
                            }
                            Box::new(c)
                        } else {
                            Box::new(Transport::connect_until(&config.endpoint, deadline)?)
                        });
                    }
                    query = query.checked_add(1).ok_or("spectrum query exhausted")?;
                    let request = serde_json::json!({"contract":"GP-SPECTRUM","version":version,"show_id":id.show,"epoch":id.epoch,"map":id.map,"query":query,"input":input});
                    let c = connection.as_mut().unwrap();
                    c.send_frame_until(
                        &serde_json::to_vec(&request).map_err(|e| e.to_string())?,
                        deadline,
                    )?;
                    let bytes = c.receive_meter_until(deadline)?.ok_or("spectrum missing")?;
                    if bytes.len() > 131072 {
                        return Err("spectrum capacity".into());
                    }
                    let v = crate::provider::parse_document(&bytes)?;
                    if v["contract"] != "GP-SPECTRUM" {
                        supported = false;
                        return Err("unsupported".into());
                    }
                    let s: Snapshot = serde_json::from_value(v).map_err(|e| e.to_string())?;
                    if let Some(pin) = c.held_identity()
                        && crate::provider::counter(&pin.map)? != id.map
                    {
                        return Err("spectrum remote map generation".into());
                    }
                    if s.version != version {
                        return Err("spectrum version negotiation".into());
                    }
                    let choices = s.sources.clone().unwrap_or_else(|| {
                        vec![Source {
                            id: input.clone(),
                            label: input.clone(),
                            tap: "raw-pre-processing".into(),
                        }]
                    });
                    discovered = s.supported;
                    cache.accept(s, &id, &input, query, began.elapsed())?;
                    *inventory.lock().unwrap() = choices;
                    *out.lock().unwrap() = cache.view();
                    Ok(())
                })();
                if result.is_err() {
                    if !discovered {
                        // One explicit v1 retry on a separate observer connection. Never send v2 source IDs to v1.
                        if version == 2 && !input.contains(':') {
                            version = 1;
                            supported = true;
                        } else {
                            supported = false;
                        }
                    }
                    connection = None;
                    cache = Cache::default();
                    *out.lock().unwrap() = None;
                }
                thread::sleep(Duration::from_millis(100).saturating_sub(began.elapsed()));
            }
        });
        Self {
            key,
            latest,
            sources,
            stop,
            child: Some(child),
        }
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
    pub fn finished(&self) -> bool {
        self.child.as_ref().is_none_or(|c| c.is_finished())
    }
    pub fn sources(&self) -> Vec<Source> {
        if self.stop.load(Ordering::Acquire) {
            return vec![];
        }
        self.sources.lock().unwrap().clone()
    }
    pub fn view(&self) -> Option<View> {
        if self.stop.load(Ordering::Acquire) {
            return None;
        }
        self.latest.lock().unwrap().clone().filter(View::fresh)
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
        if let Some(child) = self.child.take()
            && child.is_finished()
        {
            let _ = child.join();
        }
    }
}

/// Max of FFT bins whose half-bin frequency boundaries overlap each log column.
/// This is display mapping, not FFT or new independent frequency resolution.
pub fn aggregate(bins: &[i32], rate: u32, width: usize) -> Vec<i32> {
    (0..width)
        .map(|x| {
            let lo = 20. * 1000f64.powf(x as f64 / width as f64);
            let hi = 20. * 1000f64.powf((x + 1) as f64 / width as f64);
            let step = rate as f64 / 4096.;
            let first = ((lo / step - 0.5).floor() as usize + 1).max(1);
            let end = ((hi / step + 0.5).ceil() as usize).min(bins.len());
            bins.get(first..end)
                .and_then(|b| b.iter().max())
                .copied()
                .unwrap_or(-96000)
        })
        .collect()
}
