//! World draw recording into render bundles, the bind-group cache and the
//! per-pass draw census.

use super::*;

/// Test only: one entity draw of [`Renderer::ent_census`].
#[cfg(test)]
pub(crate) type EntCensus = (u32, u32, u32, usize, bool, bool);

/// A frame's draw calls by kind and its pipeline and bind group sets, in
/// the depth prepass and the main scene pass (cod4-client-perf.md 21).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct PassCensus {
    pub sky: u32,
    pub world: u32,
    pub ents: u32,
    pub gun: u32,
    pub marks: u32,
    pub fx: u32,
    pub dlights: u32,
    pub prepass_world: u32,
    pub prepass_ents: u32,
    pub pipelines: u32,
    pub bind_groups: u32,
}

impl PassCensus {
    pub fn draws(&self) -> u32 {
        self.sky
            + self.world
            + self.ents
            + self.gun
            + self.marks
            + self.fx
            + self.dlights
            + self.prepass_world
            + self.prepass_ents
    }
}

/// Where `emit_world_draw` sends its commands: the scene pass itself, or
/// a [`Recording`] that a render bundle replays (cod4-client-perf.md 31).
pub(super) trait DrawSink {
    fn pipeline(&mut self, p: &wgpu::RenderPipeline);
    fn group(&mut self, slot: u32, bg: &wgpu::BindGroup, offset: Option<u32>);
    fn vertices(&mut self, slot: u32, s: wgpu::BufferSlice<'_>);
    fn indices(&mut self, s: wgpu::BufferSlice<'_>, f: wgpu::IndexFormat);
    fn draw(&mut self, indices: std::ops::Range<u32>);
    /// An entity surface's draw: its vertices from `base`, one instance.
    fn draw_inst(&mut self, indices: std::ops::Range<u32>, base: i32, instance: u32);
}

impl DrawSink for wgpu::RenderPass<'_> {
    fn pipeline(&mut self, p: &wgpu::RenderPipeline) {
        self.set_pipeline(p);
    }
    fn group(&mut self, slot: u32, bg: &wgpu::BindGroup, offset: Option<u32>) {
        self.set_bind_group(slot, bg, offset.as_slice());
    }
    fn vertices(&mut self, slot: u32, s: wgpu::BufferSlice<'_>) {
        self.set_vertex_buffer(slot, s);
    }
    fn indices(&mut self, s: wgpu::BufferSlice<'_>, f: wgpu::IndexFormat) {
        self.set_index_buffer(s, f);
    }
    fn draw(&mut self, indices: std::ops::Range<u32>) {
        self.draw_indexed(indices, 0, 0..1);
    }
    fn draw_inst(&mut self, indices: std::ops::Range<u32>, base: i32, instance: u32) {
        self.draw_indexed(indices, base, instance..instance + 1);
    }
}

/// One command of a [`Recording`].
#[derive(PartialEq)]
pub(super) enum Cmd {
    Pipeline(wgpu::RenderPipeline),
    Group(u32, wgpu::BindGroup, Option<u32>),
    Vertices(u32, wgpu::Buffer, u64, u64),
    Indices(wgpu::Buffer, wgpu::IndexFormat, u64, u64),
    Draw(std::ops::Range<u32>),
    DrawInst(std::ops::Range<u32>, i32, u32),
}

/// A run of world draws' commands, recorded every frame (an `Arc` clone
/// a handle, no validation) and compared with the last frame's: the
/// same commands replay last frame's bundle.
#[derive(Default, PartialEq)]
pub(super) struct Recording(Vec<Cmd>);

impl DrawSink for Recording {
    fn pipeline(&mut self, p: &wgpu::RenderPipeline) {
        self.0.push(Cmd::Pipeline(p.clone()));
    }
    fn group(&mut self, slot: u32, bg: &wgpu::BindGroup, offset: Option<u32>) {
        self.0.push(Cmd::Group(slot, bg.clone(), offset));
    }
    fn vertices(&mut self, slot: u32, s: wgpu::BufferSlice<'_>) {
        self.0.push(Cmd::Vertices(
            slot,
            s.buffer().clone(),
            s.offset(),
            s.size(),
        ));
    }
    fn indices(&mut self, s: wgpu::BufferSlice<'_>, f: wgpu::IndexFormat) {
        self.0
            .push(Cmd::Indices(s.buffer().clone(), f, s.offset(), s.size()));
    }
    fn draw(&mut self, indices: std::ops::Range<u32>) {
        self.0.push(Cmd::Draw(indices));
    }
    fn draw_inst(&mut self, indices: std::ops::Range<u32>, base: i32, instance: u32) {
        self.0.push(Cmd::DrawInst(indices, base, instance));
    }
}

impl Recording {
    pub(super) fn draws(&self) -> usize {
        self.0
            .iter()
            .filter(|c| matches!(c, Cmd::Draw(_) | Cmd::DrawInst(..)))
            .count()
    }

    /// The commands into `pass`, one by one.
    pub(super) fn replay(&self, pass: &mut impl DrawSink) {
        for c in &self.0 {
            match c {
                Cmd::Pipeline(p) => pass.pipeline(p),
                Cmd::Group(slot, bg, o) => pass.group(*slot, bg, *o),
                Cmd::Vertices(slot, b, o, n) => pass.vertices(*slot, b.slice(*o..*o + *n)),
                Cmd::Indices(b, f, o, n) => pass.indices(b.slice(*o..*o + *n), *f),
                Cmd::Draw(r) => pass.draw(r.clone()),
                Cmd::DrawInst(r, b, i) => pass.draw_inst(r.clone(), *b, *i),
            }
        }
    }

    /// The commands in a bundle for a pass with colour target `format`
    /// (none: the depth prepass) over the scene's depth.
    pub(super) fn bundle(
        &self,
        device: &wgpu::Device,
        format: Option<wgpu::TextureFormat>,
    ) -> wgpu::RenderBundle {
        let colour = [format];
        let mut enc = device.create_render_bundle_encoder(&wgpu::RenderBundleEncoderDescriptor {
            label: Some("world run"),
            color_formats: match format {
                Some(_) => &colour,
                None => &[],
            },
            depth_stencil: Some(wgpu::RenderBundleDepthStencil {
                format: DEPTH_FORMAT,
                depth_read_only: false,
                stencil_read_only: false,
            }),
            sample_count: msaa_samples(),
            multiview: None,
        });
        for c in &self.0 {
            match c {
                Cmd::Pipeline(p) => enc.set_pipeline(p),
                Cmd::Group(slot, bg, o) => enc.set_bind_group(*slot, bg, o.as_slice()),
                Cmd::Vertices(slot, b, o, n) => enc.set_vertex_buffer(*slot, b.slice(*o..*o + *n)),
                Cmd::Indices(b, f, o, n) => enc.set_index_buffer(b.slice(*o..*o + *n), *f),
                Cmd::Draw(r) => enc.draw_indexed(r.clone(), 0, 0..1),
                Cmd::DrawInst(r, b, i) => enc.draw_indexed(r.clone(), *b, *i..*i + 1),
            }
        }
        enc.finish(&wgpu::RenderBundleDescriptor {
            label: Some("world run"),
        })
    }
}

/// A pass's runs of draws, in frame order, each with its bundle once its
/// commands held from one frame to the next (cod4-client-perf.md 31, 40):
/// the scene pass's world runs and entity runs, the depth prepass's world
/// and entities. A run whose commands change every frame (a moving view,
/// a body entering or leaving it) replays into the pass: a bundle made
/// for one frame costs more than it saves.
#[derive(Default)]
pub(super) struct WorldBundles {
    pub(super) runs: Vec<(Recording, Option<wgpu::RenderBundle>)>,
    /// The targets the bundles were made for: colour format (none in a
    /// depth-only prepass), samples.
    pub(super) targets: Option<(Option<wgpu::TextureFormat>, u32)>,
    /// This frame's runs so far.
    pub(super) at: usize,
}

impl WorldBundles {
    pub(super) fn begin(&mut self, format: Option<wgpu::TextureFormat>) {
        let targets = Some((format, msaa_samples()));
        if self.targets != targets {
            self.runs.clear();
            self.targets = targets;
        }
        self.at = 0;
    }

    /// This frame's next run, recorded in `rec`, into `pass`: last frame's
    /// run at the same place held the same commands, its bundle (made now
    /// if it was new then), else the commands one by one. A bundled run's
    /// draws count under `kind`.
    pub(super) fn draw(
        &mut self,
        pass: &mut wgpu::RenderPass<'_>,
        device: &wgpu::Device,
        rec: Recording,
        kind: crate::frame_prof::Count,
    ) {
        use crate::frame_prof::{Count, count};
        let format = self.targets.and_then(|t| t.0);
        let at = self.at;
        self.at += 1;
        match self.runs.get_mut(at) {
            Some((old, bundle)) if *old == rec => {
                if bundle.is_none() {
                    count(Count::Rebundled, 1);
                }
                count(kind, rec.draws());
                pass.execute_bundles([&*bundle.get_or_insert_with(|| rec.bundle(device, format))]);
            }
            run => {
                rec.replay(pass);
                match run {
                    Some(run) => *run = (rec, None),
                    None => self.runs.push((rec, None)),
                }
            }
        }
    }

    /// Drops the runs past this frame's last.
    pub(super) fn end(&mut self) {
        self.runs.truncate(self.at);
    }
}

/// The bind groups a run of world draws left at groups 1 and 2: each
/// one's address and dynamic offset. A pipeline set clears it, so a
/// draw rebinds only a group the last draw under its pipeline did not
/// leave (cod4-client-perf.md 21).
#[derive(Clone, Copy, Default, PartialEq)]
pub(super) struct WorldGroups([Option<(usize, u32)>; 2]);

impl WorldGroups {
    /// Binds `bg` at group `slot` (1 or 2) unless it is there already.
    pub(super) fn bind(
        &mut self,
        pass: &mut impl DrawSink,
        slot: u32,
        bg: &wgpu::BindGroup,
        offset: Option<u32>,
        census: &std::cell::Cell<PassCensus>,
    ) {
        let key = Some((std::ptr::from_ref(bg) as usize, offset.unwrap_or(u32::MAX)));
        let at = &mut self.0[slot as usize - 1];
        if *at == key {
            return;
        }
        *at = key;
        pass.group(slot, bg, offset);
        tally(census, |c| c.bind_groups += 1);
    }
}

/// Adds to a frame's [`PassCensus`].
pub(super) fn tally(c: &std::cell::Cell<PassCensus>, f: impl FnOnce(&mut PassCensus)) {
    let mut v = c.get();
    f(&mut v);
    c.set(v);
}
