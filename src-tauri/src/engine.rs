//! The engine owns every device, the current scene (what the lights should
//! show), persistent settings and profiles. The Tauri commands and the
//! `probe` CLI are thin wrappers around it.
//!
//! A worker thread per device keeps devices alive that need periodic
//! refreshes (the QuadCast mic falls back to its own lighting when the host
//! goes quiet). The same machinery will carry animated effects later.

use crate::devices::{self, Device, DeviceInfo, Effect, Render, Rgb};
use anyhow::{anyhow, Context, Result};
use hidapi::HidApi;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

/// Per-device color correction for hardware whose LEDs are miscalibrated
/// from the factory.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ColorFix {
    /// Three letters from {r,g,b}: which of OUR channels the device receives
    /// in its R, G and B slots. "rgb" is identity, "rbg" swaps green and blue.
    pub order: String,
    /// Gain per channel (our r, g, b) before remapping, 0.0 ..= 2.0.
    pub gain: [f32; 3],
}

impl Default for ColorFix {
    fn default() -> Self {
        ColorFix { order: "rgb".into(), gain: [1.0, 1.0, 1.0] }
    }
}

impl ColorFix {
    pub fn is_identity(&self) -> bool {
        self.order == "rgb" && self.gain.iter().all(|g| (*g - 1.0).abs() < 0.001)
    }

    pub fn apply(&self, c: Rgb) -> Rgb {
        let g = |v: u8, k: f32| ((v as f32) * k.clamp(0.0, 2.0)).round().clamp(0.0, 255.0) as u8;
        let src = [g(c.r, self.gain[0]), g(c.g, self.gain[1]), g(c.b, self.gain[2])];
        let pick = |ch: Option<char>| match ch {
            Some('g') => src[1],
            Some('b') => src[2],
            _ => src[0],
        };
        let mut it = self.order.chars();
        let (r, gg, b) = (pick(it.next()), pick(it.next().or(Some('g'))), pick(it.next().or(Some('b'))));
        Rgb::new(r, gg, b)
    }
}

/// User-editable hardware layout and app options.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    /// SL-Infinity fans on each of the 4 hub ports.
    pub hub_fans_per_port: [u8; 4],
    /// LEDs on the motherboard's two addressable headers (0 = nothing attached).
    pub aura_header_leds: [u16; 2],
    pub start_minimized: bool,
    /// User names for zones, keyed "device/zone-index".
    #[serde(default)]
    pub zone_names: BTreeMap<String, String>,
    /// device id -> correction
    #[serde(default)]
    pub color_fix: BTreeMap<String, ColorFix>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            hub_fans_per_port: [3, 3, 3, 0],
            aura_header_leds: [0, 0],
            start_minimized: true,
            zone_names: BTreeMap::new(),
            color_fix: BTreeMap::new(),
        }
    }
}

/// Everything needed to reproduce the lighting.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scene {
    /// 0 ..= 100
    pub brightness: u8,
    /// device id -> one color per zone
    pub colors: BTreeMap<String, Vec<Rgb>>,
    /// Devices that keep their own colors when "Everything" is changed.
    #[serde(default)]
    pub locked: Vec<String>,
}

impl Default for Scene {
    fn default() -> Self {
        Scene { brightness: 100, colors: BTreeMap::new(), locked: Vec::new() }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DeviceStatus {
    pub id: String,
    pub ok: bool,
    pub error: Option<String>,
}

type DevHandle = Arc<Mutex<Box<dyn Device>>>;

struct Entry {
    dev: DevHandle,
    info: DeviceInfo,
    /// Device needs periodic refreshes from the host.
    software_only: bool,
}

pub struct Engine {
    entries: Vec<Entry>,
    scene: Scene,
    settings: Settings,
    status: Vec<DeviceStatus>,
    started: Instant,
    /// Bumped on every rescan so workers for stale devices exit.
    generation: u64,
}

pub type Shared = Arc<Mutex<Engine>>;

/// Refresh interval for devices that need keep-alive frames.
const FRAME_MS: u64 = 40;

fn data_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("Lwnny");
    // Carry over data from the app's previous name.
    let old = base.join("RGP");
    if !dir.exists() && old.exists() {
        let _ = fs::rename(&old, &dir);
    }
    dir
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &PathBuf) -> Option<T> {
    let s = fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

fn write_json<T: Serialize>(path: &PathBuf, v: &T) -> Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    fs::write(path, serde_json::to_string_pretty(v)?)?;
    Ok(())
}

impl Engine {
    /// Load settings and the last scene from disk, then detect hardware.
    pub fn new() -> Engine {
        let dir = data_dir();
        let settings: Settings = read_json(&dir.join("settings.json")).unwrap_or_default();
        let scene: Scene = read_json(&dir.join("scene.json")).unwrap_or_default();
        let mut e = Engine {
            entries: Vec::new(),
            scene,
            settings,
            status: Vec::new(),
            started: Instant::now(),
            generation: 0,
        };
        e.rescan();
        e
    }

    /// Re-open every device. Safe to call at any time.
    pub fn rescan(&mut self) {
        self.entries.clear();
        self.status.clear();
        self.generation += 1;
        let s = self.settings.clone();

        match HidApi::new() {
            Ok(api) => {
                self.add(devices::lianli_hub::ID, devices::lianli_hub::LianLiHub::open(&api, s.hub_fans_per_port).map(|d| d.map(|d| Box::new(d) as Box<dyn Device>)));
                self.add(devices::strimer::ID, devices::strimer::Strimer::open(&api).map(|d| d.map(|d| Box::new(d) as Box<dyn Device>)));
                self.add(devices::aura::ID, devices::aura::AuraMainboard::open(&api, s.aura_header_leds).map(|d| d.map(|d| Box::new(d) as Box<dyn Device>)));
                self.add(devices::quadcast::ID, devices::quadcast::QuadCast::open(&api).map(|d| d.map(|d| Box::new(d) as Box<dyn Device>)));
            }
            Err(e) => self.status.push(DeviceStatus { id: "hid".into(), ok: false, error: Some(format!("HID init failed: {e}")) }),
        }
        self.add(devices::ene_gpu::ID, devices::ene_gpu::EneGpu::open().map(|d| d.map(|d| Box::new(d) as Box<dyn Device>)));

        self.apply_zone_names();
        self.ensure_slots();
    }

    fn add(&mut self, id: &str, res: Result<Option<Box<dyn Device>>>) {
        match res {
            Ok(Some(d)) => {
                self.status.push(DeviceStatus { id: id.into(), ok: true, error: None });
                let info = d.info().clone();
                let software_only = d.needs_software_effects();
                self.entries.push(Entry { dev: Arc::new(Mutex::new(d)), info, software_only });
            }
            Ok(None) => self.status.push(DeviceStatus { id: id.into(), ok: false, error: Some("not connected".into()) }),
            Err(e) => self.status.push(DeviceStatus { id: id.into(), ok: false, error: Some(format!("{e:#}")) }),
        }
    }

    /// Overlay the user's zone names on the driver defaults.
    fn apply_zone_names(&mut self) {
        for e in &mut self.entries {
            for (i, z) in e.info.zones.iter_mut().enumerate() {
                if let Some(n) = self.settings.zone_names.get(&format!("{}/{}", e.info.id, i)) {
                    if !n.trim().is_empty() {
                        z.name = n.trim().to_string();
                    }
                }
            }
        }
    }

    /// Make sure the scene has a color slot for every zone of every device.
    fn ensure_slots(&mut self) {
        for e in &self.entries {
            let entry = self.scene.colors.entry(e.info.id.clone()).or_default();
            entry.resize(e.info.zones.len(), Rgb::new(255, 255, 255));
        }
    }

    pub fn device_infos(&self) -> Vec<DeviceInfo> {
        self.entries.iter().map(|e| e.info.clone()).collect()
    }

    pub fn status(&self) -> &[DeviceStatus] {
        &self.status
    }

    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    fn save_settings(&self) -> Result<()> {
        write_json(&data_dir().join("settings.json"), &self.settings)
    }

    pub fn set_settings(&mut self, mut s: Settings) -> Result<()> {
        // Per-device tweaks have their own setters; keep the current ones.
        s.color_fix = self.settings.color_fix.clone();
        s.zone_names = self.settings.zone_names.clone();
        self.settings = s;
        self.save_settings()?;
        self.rescan();
        Ok(())
    }

    /// Change one device's color correction without re-opening hardware.
    pub fn set_color_fix(&mut self, device: &str, fix: ColorFix) -> Result<()> {
        if fix.is_identity() {
            self.settings.color_fix.remove(device);
        } else {
            self.settings.color_fix.insert(device.to_string(), fix);
        }
        self.save_settings()
    }

    /// Rename a zone (empty name restores the driver default).
    pub fn set_zone_name(&mut self, device: &str, zone: usize, name: &str) -> Result<()> {
        let key = format!("{device}/{zone}");
        if name.trim().is_empty() {
            self.settings.zone_names.remove(&key);
        } else {
            self.settings.zone_names.insert(key, name.trim().to_string());
        }
        self.save_settings()?;
        // Names are cosmetic; refresh them without re-opening devices.
        let defaults: Vec<DeviceInfo> = self
            .entries
            .iter()
            .map(|e| e.dev.lock().map(|d| d.info().clone()).unwrap_or_else(|_| e.info.clone()))
            .collect();
        for (e, d) in self.entries.iter_mut().zip(defaults) {
            e.info.zones = d.zones;
        }
        self.apply_zone_names();
        Ok(())
    }

    /// Flash one zone white (others off) on its device so it can be located.
    /// Call `push` afterwards to restore the scene.
    pub fn identify_zone(&self, device: &str, zone: usize) -> Result<()> {
        let e = self.entries.iter().find(|e| e.info.id == device).ok_or_else(|| anyhow!("unknown device {device}"))?;
        let mut frame = vec![Rgb::BLACK; e.info.zones.len()];
        if let Some(slot) = frame.get_mut(zone) {
            *slot = Rgb::new(255, 255, 255);
        }
        e.dev.lock().map_err(|_| anyhow!("device lock poisoned"))?.write_frame(&frame)
    }

    // ----- scene edits -------------------------------------------------

    pub fn set_zone_color(&mut self, device: &str, zone: usize, color: Rgb) -> Result<()> {
        let v = self.scene.colors.get_mut(device).ok_or_else(|| anyhow!("unknown device {device}"))?;
        let slot = v.get_mut(zone).ok_or_else(|| anyhow!("zone {zone} out of range"))?;
        *slot = color;
        Ok(())
    }

    pub fn set_device_color(&mut self, device: &str, color: Rgb) -> Result<()> {
        let v = self.scene.colors.get_mut(device).ok_or_else(|| anyhow!("unknown device {device}"))?;
        v.iter_mut().for_each(|c| *c = color);
        Ok(())
    }

    pub fn set_all_color(&mut self, color: Rgb) {
        let locked = self.scene.locked.clone();
        for (id, v) in self.scene.colors.iter_mut() {
            if locked.contains(id) {
                continue;
            }
            v.iter_mut().for_each(|c| *c = color);
        }
    }

    pub fn set_locked(&mut self, device: &str, locked: bool) {
        self.scene.locked.retain(|d| d != device);
        if locked {
            self.scene.locked.push(device.to_string());
        }
    }

    pub fn set_brightness(&mut self, b: u8) {
        self.scene.brightness = b.min(100);
    }

    pub fn replace_scene(&mut self, scene: Scene) {
        self.scene = scene;
        self.ensure_slots();
    }

    // ----- hardware ----------------------------------------------------

    fn corrected(&self, id: &str, raw: &[Rgb]) -> Vec<Rgb> {
        match self.settings.color_fix.get(id) {
            Some(fix) => raw.iter().map(|c| fix.apply(*c)).collect(),
            None => raw.to_vec(),
        }
    }

    /// Push the scene to every device and remember it on disk.
    pub fn push(&mut self) -> Vec<DeviceStatus> {
        let mut out = Vec::new();
        for e in &self.entries {
            let id = e.info.id.clone();
            let raw = self.scene.colors.get(&id).cloned().unwrap_or_default();
            let colors = self.corrected(&id, &raw);
            let r = Render { colors: &colors, effect: Effect::Static, brightness: self.scene.brightness };
            let res = e.dev.lock().map_err(|_| anyhow!("device lock poisoned")).and_then(|mut d| d.apply(&r));
            out.push(DeviceStatus { id, ok: res.is_ok(), error: res.err().map(|e| format!("{e:#}")) });
        }
        for s in &out {
            if let Some(st) = self.status.iter_mut().find(|st| st.id == s.id) {
                st.ok = s.ok;
                st.error = s.error.clone();
            }
        }
        if let Err(e) = write_json(&data_dir().join("scene.json"), &self.scene) {
            log::warn!("could not persist scene: {e}");
        }
        out
    }

    /// Ask every device that can to store the current lighting in its memory.
    pub fn save_hardware(&mut self) -> Vec<DeviceStatus> {
        let mut out = Vec::new();
        for e in &self.entries {
            if !e.info.supports_save {
                continue;
            }
            let res = e.dev.lock().map_err(|_| anyhow!("device lock poisoned")).and_then(|mut d| d.save());
            out.push(DeviceStatus { id: e.info.id.clone(), ok: res.is_ok(), error: res.err().map(|e| format!("{e:#}")) });
        }
        out
    }

    // ----- workers -----------------------------------------------------

    pub fn device_count(&self) -> usize {
        self.entries.len()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Next keep-alive frame for device `idx`, or None if it needs none.
    pub fn frame(&self, idx: usize) -> Option<(DevHandle, Vec<Rgb>)> {
        let e = self.entries.get(idx)?;
        if !e.software_only {
            return None;
        }
        let _ = self.started; // reserved for animated effects
        let zones = e.info.zones.len();
        let raw = self.scene.colors.get(&e.info.id).cloned().unwrap_or_else(|| vec![Rgb::new(255, 255, 255); zones]);
        let bright = self.scene.brightness as f32 / 100.0;
        let frame: Vec<Rgb> = self.corrected(&e.info.id, &raw).into_iter().map(|c| c.scaled(bright)).collect();
        Some((e.dev.clone(), frame))
    }

    /// Timing of one frame per device, for the probe CLI.
    pub fn bench(&mut self) -> Vec<(String, Duration, Result<()>)> {
        let mut out = Vec::new();
        for e in &self.entries {
            let frame = vec![Rgb::new(40, 40, 40); e.info.zones.len()];
            let t0 = Instant::now();
            let res = e.dev.lock().map_err(|_| anyhow!("lock")).and_then(|mut d| d.write_frame(&frame));
            out.push((e.info.id.clone(), t0.elapsed(), res));
        }
        out
    }

    // ----- profiles ----------------------------------------------------

    fn profile_path(name: &str) -> Result<PathBuf> {
        let safe: String = name.chars().filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_').collect();
        let safe = safe.trim();
        if safe.is_empty() {
            return Err(anyhow!("profile name is empty"));
        }
        Ok(data_dir().join("profiles").join(format!("{safe}.json")))
    }

    pub fn list_profiles(&self) -> Vec<String> {
        let dir = data_dir().join("profiles");
        let mut names: Vec<String> = fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    pub fn save_profile(&self, name: &str) -> Result<()> {
        write_json(&Self::profile_path(name)?, &self.scene)
    }

    pub fn load_profile(&mut self, name: &str) -> Result<()> {
        let scene: Scene = read_json(&Self::profile_path(name)?).context("profile not found")?;
        self.replace_scene(scene);
        Ok(())
    }

    pub fn delete_profile(&self, name: &str) -> Result<()> {
        fs::remove_file(Self::profile_path(name)?).context("delete profile")
    }

    /// Run `f` against one device by id (probe CLI diagnostics).
    pub fn with_device<T>(&self, id: &str, f: impl FnOnce(&mut Box<dyn Device>) -> T) -> Option<T> {
        let e = self.entries.iter().find(|e| e.info.id == id)?;
        let mut d = e.dev.lock().ok()?;
        Some(f(&mut d))
    }
}

/// Supervisor thread: keeps one worker per device alive.
pub fn spawn_ticker(shared: Shared) {
    thread::spawn(move || {
        let mut spawned_gen = 0u64;
        loop {
            let (gen, count) = match shared.lock() {
                Ok(e) => (e.generation(), e.device_count()),
                Err(_) => (0, 0),
            };
            if gen != spawned_gen {
                spawned_gen = gen;
                for idx in 0..count {
                    let shared = shared.clone();
                    thread::spawn(move || worker(shared, idx, gen));
                }
            }
            thread::sleep(Duration::from_millis(500));
        }
    });
}

/// One device's refresh loop. Exits when the engine rescans.
fn worker(shared: Shared, idx: usize, gen: u64) {
    loop {
        let t0 = Instant::now();
        let job = match shared.lock() {
            Ok(e) => {
                if e.generation() != gen {
                    return;
                }
                e.frame(idx)
            }
            Err(_) => return,
        };
        match job {
            Some((dev, frame)) => {
                if let Ok(mut d) = dev.lock() {
                    if let Err(e) = d.write_frame(&frame) {
                        log::debug!("frame failed: {e}");
                    }
                }
                let spent = t0.elapsed().as_millis() as u64;
                thread::sleep(Duration::from_millis(FRAME_MS.saturating_sub(spent).max(2)));
            }
            None => thread::sleep(Duration::from_millis(500)),
        }
    }
}
