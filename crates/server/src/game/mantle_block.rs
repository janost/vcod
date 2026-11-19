//! The entities a mantle leaves where it ends, CoD2's and CoD4's alike
//! (CoD2 lnxded 0x80f5c34, cod2-movement.md 9; `cod4_lnxded`
//! 0x80a6537..0x80a6618, cod4-pmove.md 17.4): a `G_Spawn` owned by the
//! client, its think freeing it `mantleDuration + g_mantleBlockTimeBuffer`
//! ms on. Each game keeps its own box type `B`.

use vcod_gsc::EntId;

use crate::game::script::ScriptRuntime;

/// The live blocks: each entity, the level time its think frees it and
/// the box other players' moves clip.
#[derive(Debug)]
pub struct MantleBlocks<B> {
    pub list: Vec<(EntId, i32, B)>,
}

impl<B> Default for MantleBlocks<B> {
    fn default() -> Self {
        MantleBlocks { list: Vec::new() }
    }
}

impl<B: Copy> MantleBlocks<B> {
    /// Spawns a block whose box `make` builds off its entity number, due
    /// `duration` plus `g_mantleBlockTimeBuffer` (500, 0 to 60000) ms
    /// after `now`.
    pub fn spawn(
        &mut self,
        rt: &mut ScriptRuntime,
        now: i32,
        duration: i32,
        make: impl FnOnce(u32) -> B,
    ) {
        let buffer = rt
            .host
            .cvars
            .get("g_mantleBlockTimeBuffer")
            .trim()
            .parse::<i32>()
            .unwrap_or(500)
            .clamp(0, 60000);
        match rt.with_host(|h, cx| h.ents.spawn(cx)) {
            Ok(id) => self.list.push((id, now + duration + buffer, make(id.0))),
            Err(e) => log::warn!("mantle block dropped: {e:?}"),
        }
    }

    /// The blocks whose think is due at `now`, freed (`G_FreeEntity`).
    pub fn run(&mut self, rt: &mut ScriptRuntime, now: i32) {
        let (due, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.list)
            .into_iter()
            .partition(|&(_, at, _)| at <= now);
        self.list = kept;
        for (id, _, _) in due {
            rt.host.free_entity(id);
        }
    }

    /// Every live block's box.
    pub fn boxes(&self) -> impl Iterator<Item = B> + '_ {
        self.list.iter().map(|b| b.2)
    }
}
