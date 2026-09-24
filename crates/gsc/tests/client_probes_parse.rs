//! Every client probe parses and compiles, so a syntax slip is caught
//! without booting a server on either side.

#[test]
fn every_client_probe_compiles() {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/semantics/client-probes"
    );
    let mut n = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "gsc") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let ast = vcod_gsc::parse::parse_file(&src)
            .unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
        let mut vm = vcod_gsc::Vm::new();
        vcod_gsc::compile::compile_file(&ast, "maps/mp/gametypes/probe", vm.interner_mut())
            .unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
        n += 1;
    }
    assert!(n >= 5, "found {n} client probes");
}
