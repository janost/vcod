//! The asset types a 1.7 zone carries, as the body walk
//! ([`crate::walk`]) reads them: every name and the fields later stages
//! need, the rest kept as raw bytes or dropped. One module per family;
//! each type's layout is in `docs/research/cod4-fastfile.md`.

pub mod fx;
pub mod material;
pub mod misc;
pub mod model;
pub mod sound;
pub mod ui;
pub mod weapon;
pub mod world;

use crate::walk::{Handle, W};
use crate::zone::{AssetType, Pointer};
use anyhow::{Result, bail};
use std::fmt;
use std::io::BufRead;
use std::marker::PhantomData;

/// An index into one of [`Assets`]' arrays.
pub struct Id<T>(pub u32, PhantomData<fn() -> T>);

impl<T> Id<T> {
    pub fn new(i: usize) -> Self {
        Self(i as u32, PhantomData)
    }
    pub fn index(self) -> usize {
        self.0 as usize
    }
}
impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Id<T> {}
impl<T> PartialEq for Id<T> {
    fn eq(&self, o: &Self) -> bool {
        self.0 == o.0
    }
}
impl<T> Eq for Id<T> {}
impl<T> std::hash::Hash for Id<T> {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.0.hash(h)
    }
}
impl<T> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

macro_rules! assets {
    ($($field:ident: $variant:ident($ty:ty) = $kind:ident, $load:path;)*) => {
        /// Every asset the walk met, listed or inline, in load order per
        /// type.
        #[derive(Default)]
        pub struct Assets {
            $(pub $field: Vec<$ty>,)*
            /// Techniques, which technique sets share by address.
            pub techniques: Vec<material::Technique>,
            /// The zone's script strings: a bone name in an xmodel or an
            /// xanim is an index into them (`None` where the slot was null).
            pub script_strings: Vec<Option<String>>,
        }

        /// An asset of any type.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum AssetRef {
            $($variant(Id<$ty>),)*
        }

        impl AssetRef {
            pub fn kind(self) -> AssetType {
                match self {
                    $(Self::$variant(_) => AssetType::$kind,)*
                }
            }
        }

        impl Assets {
            /// How many assets of `kind` the walk holds.
            pub fn count(&self, kind: AssetType) -> usize {
                match kind {
                    $(AssetType::$kind => self.$field.len(),)*
                    _ => 0,
                }
            }
        }

        $(
            impl From<Id<$ty>> for AssetRef {
                fn from(id: Id<$ty>) -> Self {
                    Self::$variant(id)
                }
            }
            impl TryFrom<AssetRef> for Id<$ty> {
                type Error = AssetRef;
                fn try_from(r: AssetRef) -> Result<Self, AssetRef> {
                    match r {
                        AssetRef::$variant(id) => Ok(id),
                        r => Err(r),
                    }
                }
            }
        )*

        /// One list entry's header: the type's `Load_*Ptr` on its slot.
        pub(crate) fn load_header<R: BufRead>(
            w: &mut W<R>,
            kind: AssetType,
            p: Pointer,
            slot: u32,
        ) -> Result<Option<AssetRef>> {
            match kind {
                $(AssetType::$kind => Ok($load(w, p, slot)?.map(AssetRef::from)),)*
                k if !k.has_loader() => Ok(None),
                k => bail!("no loader for {}", k.name()),
            }
        }
    };
}

assets! {
    phys_presets: PhysPreset(misc::PhysPreset) = PhysPreset, misc::phys_preset;
    xanims: XAnim(model::XAnim) = XAnimParts, model::xanim;
    xmodels: XModel(model::XModel) = XModel, model::xmodel;
    materials: Material(material::Material) = Material, material::material;
    techsets: TechSet(material::TechSet) = TechniqueSet, material::techset;
    images: Image(material::Image) = Image, material::image;
    sounds: Sound(sound::SoundAliasList) = Sound, sound::sound;
    snd_curves: SndCurve(sound::SndCurve) = SndCurve, sound::snd_curve;
    loaded_sounds: LoadedSound(sound::LoadedSound) = LoadedSound, sound::loaded_sound;
    clip_maps: ClipMap(world::ClipMapSp) = ClipMap, world::clip_map;
    clip_maps_pvs: ClipMapPvs(world::ClipMap) = ClipMapPvs, world::clip_map_pvs;
    com_worlds: ComWorld(world::ComWorld) = ComWorld, world::com_world;
    game_worlds_sp: GameWorldSp(world::GameWorldSp) = GameWorldSp, world::game_world_sp;
    game_worlds_mp: GameWorldMp(world::GameWorldMp) = GameWorldMp, world::game_world_mp;
    map_ents: MapEnts(world::MapEnts) = MapEnts, world::map_ents;
    gfx_worlds: GfxWorld(world::GfxWorld) = GfxWorld, world::gfx_world;
    light_defs: LightDef(misc::LightDef) = LightDef, misc::light_def;
    fonts: Font(ui::Font) = Font, ui::font;
    menu_lists: MenuList(ui::MenuList) = MenuList, ui::menu_list;
    menus: Menu(ui::Menu) = Menu, ui::menu;
    localize: Localize(misc::Localize) = Localize, misc::localize;
    weapons: Weapon(weapon::Weapon) = Weapon, weapon::weapon;
    fx: Fx(fx::Fx) = Fx, fx::fx;
    impact_fx: ImpactFx(fx::ImpactFx) = ImpactFx, fx::impact_fx;
    raw_files: RawFile(misc::RawFile) = RawFile, misc::raw_file;
    string_tables: StringTable(misc::StringTable) = StringTable, misc::string_table;
}

/// A reusable asset's `Load_*Ptr`: `load` runs on an inline body and
/// pushes the asset into its array.
pub(crate) fn asset<R: BufRead, T>(
    w: &mut W<R>,
    p: Pointer,
    slot: u32,
    how: Handle,
    load: impl FnOnce(&mut W<R>) -> Result<Id<T>>,
) -> Result<Option<Id<T>>>
where
    Id<T>: Into<AssetRef> + TryFrom<AssetRef>,
{
    let full: &'static str = std::any::type_name::<T>();
    let what = full.rsplit("::").next().unwrap_or(full);
    let r = w.handle(p, slot, how, what, |w| Ok(load(w)?.into()))?;
    Ok(match r {
        None => None,
        Some(r) => match Id::<T>::try_from(r) {
            Ok(id) => Some(id),
            Err(_) => bail!("{what} handle names a {}", r.kind().name()),
        },
    })
}

/// Pushes `v` onto `list`, returning its id.
pub(crate) fn push<T>(list: &mut Vec<T>, v: T) -> Id<T> {
    list.push(v);
    Id::new(list.len() - 1)
}
