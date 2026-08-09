//! Three channels of audio input.
//!
//! JACK on Linux, CoreAudio (through cpal) everywhere else. macOS ships no
//! JACK, so requiring it there would mean asking for a third party install
//! just to get sound into the visualiser.

use ringbuf::Producer;

/// One producer per visualised channel.
pub type Producers = [Producer<f32>; 3];

#[cfg(target_os = "linux")]
pub use self::jack_backend::{list_inputs, start, Audio};

#[cfg(not(target_os = "linux"))]
pub use self::cpal_backend::{list_inputs, start, Audio};

#[cfg(target_os = "linux")]
mod jack_backend {
    use super::Producers;
    use std::convert::TryInto;

    const PORTS: [&str; 3] = ["vis_in_1", "vis_in_2", "vis_in_3"];

    /// Keeps the JACK client alive; dropping it stops capture.
    pub struct Audio {
        _client: jack::AsyncClient<(), Proc>,
    }

    struct Proc {
        ports: [jack::Port<jack::AudioIn>; 3],
        producers: Producers,
    }

    impl jack::ProcessHandler for Proc {
        fn process(&mut self, _: &jack::Client, ps: &jack::ProcessScope) -> jack::Control {
            let mut overrun = false;
            for (port, producer) in self.ports.iter().zip(self.producers.iter_mut()) {
                let samples = port.as_slice(ps);
                if producer.push_slice(samples) != samples.len() {
                    overrun = true;
                }
            }
            if overrun {
                println!("ring buffer overrun");
            }
            jack::Control::Continue
        }
    }

    pub fn list_inputs() {
        println!("JACK: connect a source to these ports once the app is running");
        for name in PORTS.iter() {
            println!("  visualizer:{}", name);
        }
    }

    pub fn start(producers: Producers, device: Option<&str>) -> Result<Audio, String> {
        if device.is_some() {
            eprintln!("--device is ignored under JACK; connect the vis_in_* ports instead");
        }

        let (client, _status) =
            jack::Client::new("visualizer", jack::ClientOptions::NO_START_SERVER)
                .map_err(|e| format!("could not connect to JACK: {}", e))?;

        let mut ports = Vec::with_capacity(3);
        for name in PORTS.iter() {
            ports.push(
                client
                    .register_port(name, jack::AudioIn::default())
                    .map_err(|e| format!("could not register port {}: {}", name, e))?,
            );
        }
        let ports: [jack::Port<jack::AudioIn>; 3] = ports
            .try_into()
            .map_err(|_| "could not register three ports".to_string())?;

        let client = client
            .activate_async((), Proc { ports, producers })
            .map_err(|e| format!("could not activate JACK client: {}", e))?;

        Ok(Audio { _client: client })
    }
}

#[cfg(not(target_os = "linux"))]
mod cpal_backend {
    use super::Producers;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    /// Keeps the input stream alive; dropping it stops capture.
    pub struct Audio {
        _stream: cpal::Stream,
    }

    pub fn list_inputs() {
        let host = cpal::default_host();
        let default = host
            .default_input_device()
            .and_then(|d| d.name().ok())
            .unwrap_or_default();

        match host.input_devices() {
            Ok(devices) => {
                println!("input devices:");
                for device in devices {
                    let name = device.name().unwrap_or_else(|_| "<unknown>".into());
                    let channels = device
                        .default_input_config()
                        .map(|c| c.channels())
                        .unwrap_or(0);
                    let mark = if name == default { " (default)" } else { "" };
                    println!("  {} -- {} channel(s){}", name, channels, mark);
                }
            }
            Err(e) => eprintln!("could not list input devices: {}", e),
        }
    }

    pub fn start(producers: Producers, device: Option<&str>) -> Result<Audio, String> {
        let host = cpal::default_host();

        let device = match device {
            Some(wanted) => host
                .input_devices()
                .map_err(|e| format!("could not enumerate input devices: {}", e))?
                .find(|d| {
                    d.name()
                        .map(|n| n.to_lowercase().contains(&wanted.to_lowercase()))
                        .unwrap_or(false)
                })
                .ok_or_else(|| format!("no input device matching {:?}", wanted))?,
            None => host
                .default_input_device()
                .ok_or_else(|| "no default input device".to_string())?,
        };

        let name = device.name().unwrap_or_else(|_| "<unknown>".into());
        let supported = device
            .default_input_config()
            .map_err(|e| format!("no input config for {}: {}", name, e))?;
        let channels = supported.channels() as usize;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();

        println!(
            "audio input: {} ({} channel(s), {} Hz, {})",
            name, channels, config.sample_rate.0, format
        );
        if channels < 3 {
            eprintln!(
                "note: {} only has {} channel(s); the plot needs 3, so the missing \
                 ones reuse the last available channel. Use --device with a 3+ channel \
                 interface, or a virtual device such as BlackHole, for a real plot.",
                name, channels
            );
        }

        let err_fn = |e| eprintln!("audio stream error: {}", e);

        // CoreAudio always hands out f32, but other hosts do not.
        macro_rules! build {
            ($t:ty) => {{
                let mut producers = producers;
                device.build_input_stream(
                    &config,
                    move |data: &[$t], _: &cpal::InputCallbackInfo| {
                        feed(data, channels, &mut producers)
                    },
                    err_fn,
                    None,
                )
            }};
        }

        let stream = match format {
            cpal::SampleFormat::F32 => build!(f32),
            cpal::SampleFormat::F64 => build!(f64),
            cpal::SampleFormat::I8 => build!(i8),
            cpal::SampleFormat::I16 => build!(i16),
            cpal::SampleFormat::I32 => build!(i32),
            cpal::SampleFormat::U8 => build!(u8),
            cpal::SampleFormat::U16 => build!(u16),
            cpal::SampleFormat::U32 => build!(u32),
            other => return Err(format!("unsupported sample format {}", other)),
        }
        .map_err(|e| format!("could not open input stream on {}: {}", name, e))?;

        stream
            .play()
            .map_err(|e| format!("could not start input stream: {}", e))?;

        Ok(Audio { _stream: stream })
    }

    /// De-interleaves one callback buffer into the three ring buffers. Channels
    /// beyond what the device offers repeat the last one, so a stereo or mono
    /// device still produces a picture instead of failing outright.
    fn feed<T>(data: &[T], channels: usize, producers: &mut Producers)
    where
        T: cpal::Sample,
        f32: cpal::FromSample<T>,
    {
        if channels == 0 {
            return;
        }
        for frame in data.chunks(channels) {
            if frame.is_empty() {
                continue;
            }
            for (i, producer) in producers.iter_mut().enumerate() {
                let sample = frame[i.min(frame.len() - 1)];
                let _ = producer.push(f32::from_sample_(sample));
            }
        }
    }
}
