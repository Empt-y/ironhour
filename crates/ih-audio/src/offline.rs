//! Real mixer output without opening a sound device, for CI and review WAVs.
use super::*;
use kira::backend::{Backend, Renderer};

pub struct Offline {
    renderer: Option<Renderer>,
}
impl Backend for Offline {
    type Settings = ();
    type Error = std::convert::Infallible;
    fn setup(_: (), _: usize) -> Result<(Self, u32), Self::Error> {
        Ok((Self { renderer: None }, 48000))
    }
    fn start(&mut self, renderer: Renderer) -> Result<(), Self::Error> {
        self.renderer = Some(renderer);
        Ok(())
    }
}
impl Audio<Offline> {
    pub fn offline(catalog: Catalog) -> Self {
        Self::with_manager(catalog, AudioManager::new(AudioManagerSettings::default()).unwrap())
    }
    pub fn render_buffer(&mut self, vfs: &Vfs, now: Duration) -> Result<Vec<f32>> {
        let errors = self.update_media(vfs, now);
        ensure!(errors.is_empty(), "audio render: {}", errors.join("; "));
        let renderer = self.manager.backend_mut().renderer.as_mut().unwrap();
        renderer.on_start_processing();
        let mut frames = vec![0.0; 256];
        renderer.process(&mut frames, 2);
        Ok(frames)
    }
}

pub fn write_wav(path: &std::path::Path, samples: &[f32]) -> Result<()> {
    use std::io::Write;
    let bytes = u32::try_from(samples.len().checked_mul(2).context("WAV too large")?)?;
    let mut file = std::fs::File::create(path)?;
    file.write_all(b"RIFF")?;
    file.write_all(&(bytes + 36).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&2u16.to_le_bytes())?;
    file.write_all(&48000u32.to_le_bytes())?;
    file.write_all(&192000u32.to_le_bytes())?;
    file.write_all(&4u16.to_le_bytes())?;
    file.write_all(&16u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&bytes.to_le_bytes())?;
    for sample in samples {
        file.write_all(&((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixer_produces_stereo_samples_and_live_mute_is_silent() {
        let (_temp, vfs) = crate::tests::files(false);
        let defs = crate::tests::definitions(
            "AudioEvent Engine\n Sounds = click\n Type = world\n Control = loop\n LoopCount = 0\nEnd\n",
        );
        let mut a = Audio::offline(Catalog::load(&defs, "English"));
        a.emitter(&vfs, 1, "Engine", [50.0, 0.0, 0.0]).unwrap();
        let mut right = 0.0f32;
        let mut left = 0.0f32;
        for i in 0..30 {
            let samples = a.render_buffer(&vfs, Duration::from_millis(i * 3)).unwrap();
            for pair in samples.as_chunks::<2>().0 {
                left += pair[0].abs();
                right += pair[1].abs();
            }
        }
        assert!(right > 1.0 && left < right * 0.01, "right-positioned emitter must pan right");
        a.set_preferences(preferences::Preferences { effects: 0, ..Default::default() }).unwrap();
        let mut tail = vec![];
        for i in 30..60 {
            tail = a.render_buffer(&vfs, Duration::from_millis(i * 3)).unwrap();
        }
        assert!(tail.iter().all(|s| s.abs() < 0.00001), "muting must silence actual mixer samples");
    }
}
