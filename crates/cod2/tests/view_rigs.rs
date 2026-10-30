//! Every stock viewmodel and viewhands xmodel (LOD 0 type `2` or `4`)
//! skins against a bind its blend weights agree on once
//! `vcod_cod2::model::recover_bind` has rebuilt it from the per-weight
//! offsets (cod2-xmodel-v20-format.md, "Viewmodels and hands"). Needs
//! `COD2_DIR`; passes vacuously without it.
use vcod_cod2::{model, xmodel};
use vcod_common::testing::cod2_fs;

#[test]
fn view_rigs_agree_on_the_recovered_bind() {
    let Some(fs) = cod2_fs() else {
        return;
    };
    let mut rigs = 0;
    let mut hands = 0;
    for path in fs.list_prefix("xmodel/") {
        let Some(data) = fs.read(&path) else { continue };
        let mut parts = None;
        let Ok(desc) = xmodel::parse_descriptor(&data, |lod0| {
            let p = xmodel::parse_parts(&fs.read(&format!("xmodelparts/{lod0}")).unwrap())?;
            let n = p.bones.len();
            parts = Some(p);
            Ok(n)
        }) else {
            continue;
        };
        let lod0 = &desc.lods[0].name;
        if !model::is_view_rig(lod0) {
            continue;
        }
        let surfs = xmodel::parse_surfs(&fs.read(&format!("xmodelsurfs/{lod0}")).unwrap()).unwrap();
        let mut pose = xmodel::base_pose(&parts.unwrap());
        let before = model::bind_spread(&pose, &surfs);
        model::recover_bind(&mut pose, &surfs);
        let after = model::bind_spread(&pose, &surfs);
        println!("{path} ({lod0}): spread {before:.3} -> {after:.3}");
        assert!(
            after < 0.1,
            "{path}: weights disagree by {after} on the recovered bind"
        );
        assert!(pose.iter().all(|m| m.trans.is_finite()));
        rigs += 1;
        if lod0.ends_with('4') {
            hands += 1;
            assert!(
                before > 1.0,
                "{path}: the stored bind already agreed ({before})"
            );
        }
    }
    assert!(rigs >= 40 && hands >= 8, "{rigs} rigs, {hands} hands");
}
