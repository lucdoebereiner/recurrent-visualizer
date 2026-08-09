//! Three channels of audio input.
//!
//! JACK on Linux, CoreAudio (through cpal) everywhere else. macOS ships no
//! JACK, so requiring it there would mean asking for a third party install
//! just to get sound into the visualiser.

use ringbuf::Producer;

/// One producer per visualised channel.
pub type Producers = [Producer<f32>; 3];

/// Which device channels feed the three plot inputs, as 0-based indices.
/// `None` means "the first three".
pub type ChannelMap = Option<[usize; 3]>;

/// Parses a `--channels 4,5,6` argument into 0-based indices.
pub fn parse_channels(s: &str) -> Option<[usize; 3]> {
    let parsed: Option<Vec<usize>> = s.split(',').map(|p| p.trim().parse().ok()).collect();
    let parsed = parsed?;
    if parsed.len() != 3 || parsed.iter().any(|&c| c == 0) {
        return None;
    }
    Some([parsed[0] - 1, parsed[1] - 1, parsed[2] - 1])
}

#[cfg(target_os = "linux")]
pub use self::jack_backend::{list_inputs, start, Audio};

#[cfg(not(target_os = "linux"))]
pub use self::cpal_backend::{list_inputs, start, Audio};

#[cfg(test)]
mod tests {
    use super::parse_channels;

    #[test]
    fn parses_one_based_triples() {
        assert_eq!(parse_channels("1,2,3"), Some([0, 1, 2]));
        assert_eq!(parse_channels("4,5,6"), Some([3, 4, 5]));
        assert_eq!(parse_channels(" 4 , 5 , 6 "), Some([3, 4, 5]));
        // Repeats are legitimate: plotting one channel against itself.
        assert_eq!(parse_channels("2,2,7"), Some([1, 1, 6]));
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in ["", "1,2", "1,2,3,4", "0,1,2", "a,b,c", "1;2;3", "-1,2,3", "1,,3"] {
            assert_eq!(parse_channels(bad), None, "{:?} should be rejected", bad);
        }
    }
}

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

    pub fn start(
        producers: Producers,
        device: Option<&str>,
        channels: super::ChannelMap,
    ) -> Result<Audio, String> {
        if device.is_some() || channels.is_some() {
            eprintln!(
                "--device/--channels are ignored under JACK; patch the vis_in_* ports instead"
            );
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
    use cpal::FromSample;

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

    pub fn start(
        producers: Producers,
        device: Option<&str>,
        channel_map: super::ChannelMap,
    ) -> Result<Audio, String> {
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

        // An explicit request is honoured strictly: silently substituting a
        // different channel would produce a plot that looks plausible and is
        // wrong. Without a request, fall back to the first three and repeat the
        // last one, so a built in mic still draws something.
        let map: [usize; 3] = match channel_map {
            Some(requested) => {
                for &c in requested.iter() {
                    if c >= channels {
                        return Err(format!(
                            "--channels asks for channel {}, but {} has only {} channel(s)",
                            c + 1,
                            name,
                            channels
                        ));
                    }
                }
                requested
            }
            None => {
                let last = channels.saturating_sub(1);
                if channels < 3 {
                    eprintln!(
                        "note: {} only has {} channel(s); the plot needs 3, so the missing \
                         ones reuse the last available channel. Use a 3+ channel interface \
                         or a virtual device such as BlackHole for a real plot.",
                        name, channels
                    );
                }
                [last.min(0), last.min(1), last.min(2)]
            }
        };

        println!(
            "channel mapping: {} -> plot 1, {} -> plot 2, {} -> plot 3",
            map[0] + 1,
            map[1] + 1,
            map[2] + 1
        );

        let err_fn = |e| eprintln!("audio stream error: {}", e);

        // CoreAudio always hands out f32, but other hosts do not.
        macro_rules! build {
            ($t:ty) => {{
                let mut producers = producers;
                device.build_input_stream(
                    &config,
                    move |data: &[$t], _: &cpal::InputCallbackInfo| {
                        feed(data, channels, map, &mut producers)
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

    /// De-interleaves one callback buffer into the three ring buffers, taking
    /// the device channels named by `map` (already validated against
    /// `channels`).
    fn feed<T>(data: &[T], channels: usize, map: [usize; 3], producers: &mut Producers)
    where
        T: cpal::Sample,
        f32: FromSample<T>,
    {
        if channels == 0 {
            return;
        }
        for frame in data.chunks(channels) {
            // A trailing partial frame would make the map indices unsafe.
            if frame.len() < channels {
                continue;
            }
            for (i, producer) in producers.iter_mut().enumerate() {
                let _ = producer.push(f32::from_sample_(frame[map[i]]));
            }
        }
    }
}
