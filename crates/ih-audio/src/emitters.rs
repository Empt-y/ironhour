use super::*;

impl<B: Backend> Audio<B> {
    pub fn has_emitter(&self, id: u32) -> bool {
        self.active.iter().any(|v| v.emitter == Some(id))
    }

    pub fn set_time_of_day(&mut self, name: &str) {
        self.time_of_day = name.to_ascii_lowercase();
    }

    /// Progress attack -> body -> decay; zero LoopCount means an infinite body.
    pub fn advance_voices(&mut self, vfs: &Vfs) -> Result<()> {
        let voices = std::mem::take(&mut self.active);
        let mut error = None;
        for mut voice in voices {
            if voice.handle.state() != PlaybackState::Stopped {
                self.active.push(voice);
                continue;
            }
            let Some(mut event) = self.catalog.events.get(&voice.name).cloned() else { continue };
            if let Some(sounds) = event.variants.get(&format!("sounds{}", self.time_of_day)).filter(|s| !s.is_empty()) {
                event.sounds = sounds.clone();
            }
            let next = match voice.phase {
                Phase::Attack => Some(Phase::Body),
                Phase::Body if event.all && voice.body_index + 1 < event.sounds.len() => {
                    voice.body_index += 1;
                    Some(Phase::Body)
                }
                Phase::Body if event.looping && voice.remaining != 1 => {
                    voice.remaining = voice.remaining.saturating_sub(1);
                    voice.body_index = 0;
                    Some(Phase::Body)
                }
                Phase::Body if !event.decay.is_empty() => Some(Phase::Decay),
                _ => None,
            };
            let Some(phase) = next else { continue };
            let clips = match phase {
                Phase::Attack => &event.attack[..],
                Phase::Body if event.all => &event.sounds[voice.body_index..voice.body_index + 1],
                Phase::Body => &event.sounds[..],
                Phase::Decay => &event.decay[..],
            };
            match self.segment(
                vfs,
                &voice.name,
                &event,
                clips,
                voice.spatial,
                phase == Phase::Body && (event.post_delay || voice.phase == Phase::Body),
            ) {
                Ok((handle, gain)) => {
                    voice.handle = handle;
                    voice.gain = gain;
                    voice.phase = phase;
                    self.active.push(voice);
                }
                Err(e) => {
                    if error.is_none() {
                        error = Some(e);
                    }
                }
            }
        }
        error.map_or(Ok(()), Err)
    }

    /// Synchronize a drawable's moving emitter. Range-culling releases the voice;
    /// returning into range can restart it. Changing damage state changes its event.
    pub fn emitter(&mut self, vfs: &Vfs, id: u32, name: &str, position: [f32; 3]) -> Result<()> {
        let event = self.catalog.events.get(name).with_context(|| format!("unknown ambient sound: {name}"))?;
        let range = if event.global { self.catalog.global_range } else { event.range };
        if self.listener.spatial(position, range).0 <= 0.0 {
            self.stop_emitter(vfs, id, false)?;
            return Ok(());
        }
        if let Some(voice) =
            self.active.iter_mut().find(|v| v.emitter == Some(id) && v.name == name && v.phase != Phase::Decay)
        {
            voice.spatial = Some((position, range));
            return Ok(());
        }
        self.stop_emitter(vfs, id, true)?;
        if self.play_world(vfs, name, position)?
            && let Some(voice) = self.active.last_mut()
        {
            voice.emitter = Some(id);
        }
        Ok(())
    }

    pub fn stop_emitter(&mut self, vfs: &Vfs, id: u32, decay: bool) -> Result<()> {
        let voices = std::mem::take(&mut self.active);
        let mut error = None;
        for mut voice in voices {
            if voice.emitter != Some(id) {
                self.active.push(voice);
                continue;
            }
            voice.handle.stop(kira::Tween::default());
            if decay
                && voice.phase != Phase::Decay
                && let Some(event) = self.catalog.events.get(&voice.name).cloned().filter(|e| !e.decay.is_empty())
            {
                match self.segment(vfs, &voice.name, &event, &event.decay, voice.spatial, false) {
                    Ok((handle, gain)) => {
                        voice.handle = handle;
                        voice.gain = gain;
                        voice.phase = Phase::Decay;
                        voice.emitter = None;
                        self.active.push(voice);
                    }
                    Err(e) => {
                        error = Some(e);
                    }
                }
            }
        }
        error.map_or(Ok(()), Err)
    }
}
