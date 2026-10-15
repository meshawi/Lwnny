//! Command-line diagnostics. Exercises every driver on the real hardware
//! without the UI. Handy when adding a device or chasing a wiring question.
//!
//!   probe hid                       list all HID devices Windows exposes
//!   probe list                      detect devices and print zones
//!   probe set <device|all> <hex>    static color (device id from `list`)
//!   probe zone <device> <n> <hex>   static color on one zone
//!   probe hubmap                    light every Lian Li hub channel a different color
//!   probe leds <device> <total> <from-to:hex> ...   per-LED test (ENE GPU only)
//!   probe bench                     time one frame per device
//!   probe dump                      register dump for devices that support it
//!   probe save                      store current lighting in device memory
//!   probe off                       everything black

use lwnny_lib::devices::Rgb;
use lwnny_lib::engine::Engine;
use std::env;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args: Vec<String> = env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("list");

    if cmd == "hid" {
        let api = hidapi::HidApi::new().expect("hidapi");
        for d in api.device_list() {
            println!(
                "{:04X}:{:04X} if={} usage={:04X}/{:04X} {:?} {:?}",
                d.vendor_id(),
                d.product_id(),
                d.interface_number(),
                d.usage_page(),
                d.usage(),
                d.manufacturer_string().unwrap_or(""),
                d.product_string().unwrap_or("")
            );
        }
        return;
    }

    let mut engine = Engine::new();
    println!("--- devices ---");
    for s in engine.status() {
        println!("{:<12} {}", s.id, if s.ok { "OK".to_string() } else { format!("-- {}", s.error.clone().unwrap_or_default()) });
    }
    for info in engine.device_infos() {
        println!("\n[{}] {} ({})  save={}  {}", info.id, info.name, info.kind, info.supports_save, info.note);
        for (i, z) in info.zones.iter().enumerate() {
            println!("   zone {:<2} {:<40} {} LEDs", i, z.name, z.leds);
        }
    }
    println!();

    match cmd {
        "list" => {}
        "set" => {
            let target = args.get(1).map(String::as_str).unwrap_or("all");
            let color = args.get(2).and_then(|h| Rgb::from_hex(h)).unwrap_or(Rgb::new(255, 255, 255));
            engine.set_brightness(100);
            if target == "all" {
                engine.set_all_color(color);
            } else {
                engine.set_device_color(target, color).expect("device id");
            }
            report(engine.push());
        }
        "zone" => {
            let dev = args.get(1).expect("device id");
            let zone: usize = args.get(2).and_then(|s| s.parse().ok()).expect("zone index");
            let color = args.get(3).and_then(|h| Rgb::from_hex(h)).unwrap_or(Rgb::new(255, 255, 255));
            engine.set_zone_color(dev, zone, color).expect("zone");
            report(engine.push());
        }
        "hubmap" => {
            const PALETTE: [(&str, Rgb); 8] = [
                ("red", Rgb::new(255, 0, 0)),
                ("green", Rgb::new(0, 255, 0)),
                ("blue", Rgb::new(0, 0, 255)),
                ("yellow", Rgb::new(255, 180, 0)),
                ("cyan", Rgb::new(0, 255, 255)),
                ("magenta", Rgb::new(255, 0, 255)),
                ("white", Rgb::new(150, 150, 150)),
                ("orange", Rgb::new(255, 60, 0)),
            ];
            engine.set_brightness(100);
            for (i, (name, c)) in PALETTE.iter().enumerate() {
                println!("hub zone {i} -> {name}");
                engine.set_zone_color("lianli_hub", i, *c).expect("hub zone");
            }
            report(engine.push());
        }
        "leds" => {
            // probe leds <device> <total> <from-to:hex> [<from-to:hex> ...]   e.g. leds gpu 22 0-10:ff0000 11-21:0000ff
            let dev = args.get(1).expect("device id");
            let total: usize = args.get(2).and_then(|s| s.parse().ok()).expect("total LED count");
            let mut leds = vec![Rgb::BLACK; total];
            for spec in args.iter().skip(3) {
                let (range, hex) = spec.split_once(':').expect("range:hex");
                let (a, b) = range.split_once('-').unwrap_or((range, range));
                let (a, b): (usize, usize) = (a.parse().expect("from"), b.parse().expect("to"));
                let c = Rgb::from_hex(hex).expect("hex");
                for i in a..=b.min(total - 1) {
                    leds[i] = c;
                }
            }
            match engine.with_device(dev, |d| d.write_leds(&leds)) {
                Some(Ok(())) => println!("{dev}: wrote {total} LEDs"),
                Some(Err(e)) => println!("{dev}: FAILED {e:#}"),
                None => println!("{dev}: no such device"),
            }
        }
        "bench" => {
            for (id, dt, res) in engine.bench() {
                println!("{:<12} {:>6.1} ms per frame  {}", id, dt.as_secs_f64() * 1000.0, if res.is_ok() { "ok" } else { "FAILED" });
            }
        }
        "dump" => {
            for info in engine.device_infos() {
                if let Some(Some(s)) = engine.with_device(&info.id, |d| d.debug_dump()) {
                    println!("[{}]\n{}", info.id, s);
                }
            }
        }
        "save" => {
            report(engine.push());
            report(engine.save_hardware());
        }
        "off" => {
            engine.set_all_color(Rgb::BLACK);
            report(engine.push());
        }
        other => eprintln!("unknown command {other}"),
    }
}

fn report(results: Vec<lwnny_lib::engine::DeviceStatus>) {
    for r in results {
        println!("{:<12} {}", r.id, if r.ok { "applied".to_string() } else { format!("FAILED: {}", r.error.unwrap_or_default()) });
    }
}
