//! Every `.iwi` in the stock iwds (`$COD4_DIR/main/*.iwd`, the search
//! path's view: a name two iwds share is read once) decodes as version 6,
//! wavelet ones through the tables `iw3mp.exe` carries, and each read
//! stops where the header's `fileSizeForPicmip` says
//! (`docs/research/cod4-world.md` section 5).

use std::collections::BTreeMap;
use std::sync::Mutex;
use vcod_cod4::source::Cod4Source;
use vcod_cod4::testing::cod4_install;
use vcod_common::asset_source::AssetSource;
use vcod_common::iwi::Iwi;

#[test]
fn every_stock_iwi_decodes() {
    let Some(install) = cod4_install() else {
        return;
    };
    let src = Cod4Source::open(&install).unwrap();
    let names = src.iwds().list("", ".iwi");
    assert!(names.len() > 6500, "{} iwis", names.len());
    let next = Mutex::new(names.iter());
    let by_format = Mutex::new(BTreeMap::<(u8, bool), usize>::new());
    let failed = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                loop {
                    let Some(name) = next.lock().unwrap().next() else {
                        break;
                    };
                    let data = src.iwds().read(name).unwrap();
                    let r = Iwi::parse_v6(&data).and_then(|iwi| {
                        let h = iwi.header;
                        let ends = h.level_ends;
                        // fileSizeForPicmip[k]: the file's size with k
                        // levels dropped, as far as there are levels.
                        for (k, &end) in iwi.read_ends.iter().take(4).enumerate() {
                            anyhow::ensure!(
                                end == ends[k] as usize,
                                "level {k} ends at {end}, header says {}",
                                ends[k]
                            );
                        }
                        anyhow::ensure!(iwi.read_ends[0] == data.len(), "trailing bytes");
                        let _ = iwi.to_image();
                        Ok((data[4], h.is_cube()))
                    });
                    match r {
                        Ok(k) => *by_format.lock().unwrap().entry(k).or_default() += 1,
                        Err(e) => failed.lock().unwrap().push(format!("{name}: {e:#}")),
                    }
                }
            });
        }
    });
    println!(
        "{} iwis by (format, cube): {:?}",
        names.len(),
        by_format.lock().unwrap()
    );
    let failed = failed.into_inner().unwrap();
    assert!(
        failed.is_empty(),
        "{} failed: {:#?}",
        failed.len(),
        &failed[..failed.len().min(20)]
    );
}
