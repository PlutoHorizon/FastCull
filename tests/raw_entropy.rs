//! Real-device acceptance of the integer entropy kernel, including edge tiles.
#[cfg(any(windows, target_os = "macos"))]
mod native {
    use fastcull::raw_entropy::{self, Image, Kind, Tile};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn tile(x: usize, y: usize, channels: usize, long: bool) -> Tile {
        let mut table = vec![0u32; if long { 4112 } else { 4096 }];
        if long {
            table[0] = 0x8000_1000;
            table[4096] = 16 << 8;
        } else {
            for category in 0..=16 {
                table[category * 128..(category + 1) * 128].fill((5 << 8) | category as u32);
            }
        }
        let mut stream = String::new();
        for i in 0..4 * channels {
            if long {
                stream.push_str("0000000000000000");
                continue;
            }
            let delta: i32 = [0, 1, -1, 127, -127, 256, -256, 32767, -32767, -32768][i % 10];
            let cat = if delta == -32768 {
                16
            } else {
                32 - delta.unsigned_abs().leading_zeros()
            };
            stream.push_str(&format!("{cat:05b}"));
            if cat != 0 && cat != 16 {
                let magnitude = if delta < 0 {
                    delta + ((1 << cat) - 1)
                } else {
                    delta
                };
                stream.push_str(&format!("{magnitude:0width$b}", width = cat as usize));
            }
        }
        while !stream.len().is_multiple_of(8) {
            stream.push('1');
        }
        let data = stream
            .as_bytes()
            .chunks(8)
            .map(|b| b.iter().fold(0u8, |a, b| (a << 1) | u8::from(*b == b'1')))
            .collect();
        Tile {
            x,
            y,
            width: 2,
            height: 2,
            channels,
            bits: 16,
            data,
            tables: vec![Arc::from(table); channels],
        }
    }

    #[test]
    #[ignore = "requires a real DX12/Metal adapter"]
    fn tiled_entropy_gpu_pixels_and_transactional_rejection() {
        let backends = if cfg!(windows) {
            wgpu::Backends::DX12
        } else {
            wgpu::Backends::METAL
        };
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });
        let adapter = instance
            .enumerate_adapters(backends)
            .into_iter()
            .min_by_key(|a| u8::from(a.get_info().vendor != 0x10de))
            .expect("native adapter");
        eprintln!("entropy adapter: {:?}", adapter.get_info());
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        let gpu = fastcull::raw_compute::Context::new(device, queue);
        for kind in [Kind::Sony, Kind::LinearDng] {
            let step = if kind == Kind::Sony { 4 } else { 2 };
            let channels = if kind == Kind::Sony { 4 } else { 3 };
            let mut image = Image {
                width: step * 2 - 1,
                height: step * 2 - 1,
                channels: if kind == Kind::Sony { 1 } else { 3 },
                kind,
                tiles: vec![
                    tile(0, 0, channels, false),
                    tile(step, 0, channels, true),
                    tile(0, step, channels, false),
                    tile(step, step, channels, false),
                ],
            };
            let expected = raw_entropy::decode_cpu(&image, &|| false).unwrap();
            let (actual, transfer) = gpu.decode_entropy(&image, &|| false).unwrap();
            assert_eq!(actual, expected);
            assert_eq!(actual[0], 32768);
            assert!(transfer.0 > 0 && transfer.1 > 0, "transfer volume recorded");
            assert!(gpu.decode_entropy(&image, &|| true).is_err());
            let checks = AtomicUsize::new(0);
            assert!(
                gpu.decode_entropy(&image, &|| checks.fetch_add(1, Ordering::Relaxed) >= 2)
                    .is_err()
            );
            image.tiles[0].data.truncate(1);
            assert!(
                gpu.decode_entropy(&image, &|| false).is_err(),
                "partial tile must never be committed"
            );
            image.tiles[0].data = vec![255; 30];
            assert!(
                gpu.decode_entropy(&image, &|| false).is_err(),
                "undefined prefix must never be committed"
            );
        }
    }
}
