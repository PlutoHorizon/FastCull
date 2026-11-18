//! Versioned editing intent, separate from expensive scene generations.
//! Scene acceptance compares exact inputs, not hash equality. Fingerprints are
//! diagnostic FNV-1a values, not cryptographic asset identity proofs.
use anyhow::{Result, ensure};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub source: String,
    pub scene_packet: String,
    pub packet: String,
    pub light_enabled: bool,
    pub color_enabled: bool,
}
impl Snapshot {
    pub fn fingerprint(&self) -> u64 {
        self.serialize()
            .bytes()
            .fold(0xcbf29ce484222325_u64, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
            })
    }
    pub fn serialize(&self) -> String {
        format!(
            "FastCullRecipe1:{}\n{}\n{}\n{}\n{}\n{}",
            crate::raw_color::PIPELINE_VERSION,
            crate::handoff::encode(&self.source),
            crate::handoff::encode(&self.scene_packet),
            crate::handoff::encode(&self.packet),
            u8::from(self.light_enabled),
            u8::from(self.color_enabled)
        )
    }
    pub fn deserialize(text: &str) -> Result<Self> {
        let fields: Vec<_> = text.split('\n').collect();
        ensure!(
            fields.len() == 6
                && fields[0] == format!("FastCullRecipe1:{}", crate::raw_color::PIPELINE_VERSION),
            "invalid recipe schema/pipeline"
        );
        ensure!(
            matches!(fields[4], "0" | "1") && matches!(fields[5], "0" | "1"),
            "invalid recipe switches"
        );
        Ok(Self {
            source: crate::handoff::decode(fields[1])?,
            scene_packet: crate::handoff::decode(fields[2])?,
            packet: crate::handoff::decode(fields[3])?,
            light_enabled: fields[4] == "1",
            color_enabled: fields[5] == "1",
        })
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Revision {
    pub session: u64,
    pub sequence: u64,
    pub fingerprint: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionedRecipe {
    pub revision: Revision,
    pub snapshot: Snapshot,
}
impl VersionedRecipe {
    pub fn serialize(&self) -> String {
        format!(
            "FastCullRecipeRecord1\n{}\n{}\n{:016x}\n{}",
            self.revision.session,
            self.revision.sequence,
            self.revision.fingerprint,
            self.snapshot.serialize()
        )
    }
    pub fn deserialize(text: &str) -> Result<Self> {
        let fields: Vec<_> = text.splitn(5, '\n').collect();
        ensure!(
            fields.len() == 5 && fields[0] == "FastCullRecipeRecord1",
            "invalid versioned recipe"
        );
        let revision = Revision {
            session: fields[1].parse()?,
            sequence: fields[2].parse()?,
            fingerprint: u64::from_str_radix(fields[3], 16)?,
        };
        let snapshot = Snapshot::deserialize(fields[4])?;
        ensure!(
            revision.sequence > 0 && revision.fingerprint == snapshot.fingerprint(),
            "recipe fingerprint mismatch"
        );
        Ok(Self { revision, snapshot })
    }
}
impl Revision {
    pub fn fields(self) -> Vec<(&'static str, String)> {
        vec![
            ("recipe_session", self.session.to_string()),
            ("recipe_revision", self.sequence.to_string()),
            (
                "recipe_fingerprint",
                crate::raw_trace::string(&format!("{:016x}", self.fingerprint)),
            ),
            (
                "recipe_pipeline",
                crate::raw_trace::string(crate::raw_color::PIPELINE_VERSION),
            ),
        ]
    }
}
#[derive(Clone, Debug)]
pub struct SceneTicket {
    pub requested: Revision,
    source: String,
    packet: String,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameProvenance {
    pub requested: Revision,
    pub displayed: Revision,
    pub generation: u64,
    pub profile_fingerprint: u64,
    pub provisional: bool,
}
impl FrameProvenance {
    pub fn fields(self) -> Vec<(&'static str, String)> {
        let mut fields = self.displayed.fields();
        fields.extend([
            (
                "requested_recipe_session",
                self.requested.session.to_string(),
            ),
            (
                "requested_recipe_revision",
                self.requested.sequence.to_string(),
            ),
            ("scene_generation", self.generation.to_string()),
            (
                "resolved_profile_fingerprint",
                self.profile_fingerprint.to_string(),
            ),
            ("provisional", self.provisional.to_string()),
        ]);
        fields
    }
}
pub struct Clock {
    revision: Revision,
    snapshot: Option<Snapshot>,
}
impl Clock {
    pub fn new(session: u64) -> Self {
        Self {
            revision: Revision {
                session,
                ..Default::default()
            },
            snapshot: None,
        }
    }
    pub fn update(&mut self, snapshot: Snapshot) -> Revision {
        if self.snapshot.as_ref() != Some(&snapshot) {
            self.revision.sequence = self
                .revision
                .sequence
                .checked_add(1)
                .expect("recipe revision exhausted");
            self.revision.fingerprint = snapshot.fingerprint();
            self.snapshot = Some(snapshot);
        }
        self.revision
    }
    pub fn revision(&self) -> Revision {
        self.revision
    }
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()
    }
    pub fn record(&self) -> Option<VersionedRecipe> {
        self.snapshot.clone().map(|snapshot| VersionedRecipe {
            revision: self.revision,
            snapshot,
        })
    }
    pub fn ticket(&self) -> SceneTicket {
        let input = self.snapshot.as_ref().expect("recipe must be initialized");
        SceneTicket {
            requested: self.revision,
            source: input.source.clone(),
            packet: input.scene_packet.clone(),
        }
    }
    pub fn accepts(&self, ticket: &SceneTicket) -> bool {
        self.snapshot.as_ref().is_some_and(|s| {
            self.revision.session == ticket.requested.session
                && s.source == ticket.source
                && s.scene_packet == ticket.packet
        })
    }
    pub fn frame(
        &self,
        ticket: &SceneTicket,
        generation: u64,
        profile_fingerprint: u64,
    ) -> Option<FrameProvenance> {
        self.accepts(ticket).then_some(FrameProvenance {
            requested: ticket.requested,
            displayed: self.revision,
            generation,
            profile_fingerprint,
            provisional: false,
        })
    }
}
/// Normalize only the display/selection controls. All camera, WB, crop, profile,
/// curve, NR and unknown XMP fields remain in the expensive scene identity.
pub fn scene_packet(packet: &str) -> Result<String> {
    let mut basic = crate::xmp::BasicAdjustments::default();
    for control in crate::xmp::BasicControl::ALL {
        basic.set(control, Some(0));
    }
    crate::xmp::patch(
        packet,
        &crate::xmp::Updates {
            exposure: Some(0),
            basic,
            rating: Some(0),
            ..Default::default()
        },
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(source: &str, wb: u32, exposure: i32) -> Snapshot {
        let packet = crate::xmp::patch(
            &crate::xmp::template(),
            &crate::xmp::Updates {
                exposure: Some(exposure),
                white_balance: Some(crate::xmp::WhiteBalance::Custom {
                    temperature: wb,
                    tint: 0,
                }),
                ..Default::default()
            },
        )
        .unwrap();
        Snapshot {
            source: source.into(),
            scene_packet: scene_packet(&packet).unwrap(),
            packet,
            light_enabled: true,
            color_enabled: true,
        }
    }
    #[test]
    fn roundtrip_and_late_wb_do_not_overwrite_new_intent() {
        let mut clock = Clock::new(42);
        let initial = snapshot("source\n相机%", 5500, 0);
        assert_eq!(
            Snapshot::deserialize(&initial.serialize()).unwrap(),
            initial
        );
        let v = clock.update(initial.clone());
        let record = clock.record().unwrap();
        assert_eq!(
            VersionedRecipe::deserialize(&record.serialize()).unwrap(),
            record
        );
        assert!(
            VersionedRecipe::deserialize(&record.serialize().replace("\n1\n", "\n0\n")).is_err()
        );
        let ticket = clock.ticket();
        assert_eq!(clock.update(initial), v);
        let exposure = clock.update(snapshot("source\n相机%", 5500, 1_000_000));
        assert!(exposure.sequence > v.sequence);
        assert_eq!(clock.frame(&ticket, 7, 11).unwrap().displayed, exposure);
        clock.update(snapshot("source\n相机%", 6500, 1_000_000));
        assert!(!clock.accepts(&ticket));
        clock.update(snapshot("different source", 5500, 0));
        assert!(!clock.accepts(&ticket));
        assert!(Snapshot::deserialize("FastCullRecipe1\ntruncated").is_err());
    }
    #[test]
    fn replay_order_and_undo_never_reuse_revision() {
        let mut clock = Clock::new(1);
        let mut previous = 0;
        for index in 0..150 {
            let v = clock.update(snapshot(
                "same",
                if index % 3 == 0 { 4500 } else { 6500 },
                index % 11 * 100_000,
            ));
            assert!(v.sequence > previous);
            previous = v.sequence;
            let bytes = clock.snapshot().unwrap().serialize();
            assert_eq!(
                Snapshot::deserialize(&bytes).unwrap(),
                *clock.snapshot().unwrap()
            );
        }
    }
}
