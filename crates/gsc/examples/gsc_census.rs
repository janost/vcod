//! Parses and compiles every `.gsc` under a directory (a pak or iwd unpacked
//! to disk) and prints one `FAIL` line per script vcod's compiler rejects,
//! one `BUILTIN` line per (name, file) call it leaves to the host, and a
//! `SUMMARY`. `DUMP=1` prints each function's bytecode too. `DIALECT=cod2`
//! (or `cod4`)
//! compiles CoD2 script, resolving each `#include` against the same
//! directory; `BUILTINS=<file>` (one name per line) is the builtin table
//! that beats an included function, and an `UNKNOWN` line names each call
//! that is neither a script function nor in the table.
//!
//!     cargo run -p vcod-gsc --example gsc_census -- <dir>
//!
//! docs/research/cod2-gsc-diff.md section 3 is CoD2 1.3's corpus through it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use vcod_gsc::Dialect;

fn walk(d: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(d).expect("read dir") {
        let p = e.expect("dir entry").path();
        if p.is_dir() {
            walk(&p, out)
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("gsc")) {
            out.push(p)
        }
    }
}

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).expect("usage: gsc_census <dir>"));
    let dump = std::env::var_os("DUMP").is_some();
    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    let dialect = match std::env::var("DIALECT").as_deref() {
        Ok("cod2") => Dialect::Cod2,
        Ok("cod4") => Dialect::Cod4,
        _ => Dialect::Cod1,
    };
    let builtins: Option<HashSet<String>> = std::env::var_os("BUILTINS").map(|f| {
        std::fs::read_to_string(f)
            .expect("read BUILTINS")
            .lines()
            .map(|l| l.trim().to_ascii_lowercase())
            .filter(|l| !l.is_empty())
            .collect()
    });
    let rel_of = |p: &Path| {
        p.strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    };
    let by_canon: HashMap<String, PathBuf> = files
        .iter()
        .map(|p| (vcod_gsc::canonical(&rel_of(p)), p.clone()))
        .collect();
    let read =
        |p: &Path| String::from_utf8_lossy(&std::fs::read(p).expect("read script")).into_owned();
    let include_funcs = |canon: &str| {
        let src = read(by_canon.get(canon)?);
        let names = vcod_gsc::parse::parse_file_dialect(&src, dialect)
            .map(|f| f.funcs.into_iter().map(|f| f.name).collect())
            .unwrap_or_default();
        Some(names)
    };
    let is_builtin = |n: &str| builtins.as_ref().is_some_and(|b| b.contains(n));
    let opts = vcod_gsc::CompileOptions {
        dialect,
        include_funcs: &include_funcs,
        is_builtin: builtins
            .as_ref()
            .map(|_| &is_builtin as &dyn Fn(&str) -> bool),
    };
    let mut interner = vcod_gsc::Interner::default();
    let (mut ok, mut fns) = (0, 0);
    for p in &files {
        let rel = rel_of(p);
        let src = read(p);
        let ast = match vcod_gsc::parse::parse_file_dialect(&src, dialect) {
            Ok(a) => a,
            Err(e) => {
                println!("FAIL\t{rel}:{}\tparse\t{}", e.line, e.msg);
                continue;
            }
        };
        let canon = vcod_gsc::canonical(&rel);
        match vcod_gsc::compile::compile_file_with(&ast, &canon, &mut interner, &opts) {
            Ok(f) => {
                let mut bad = false;
                for func in &f {
                    if let Err(e) = vcod_gsc::bytecode::stack_depth(func) {
                        println!("FAIL\t{rel}\tstack\t{e}");
                        bad = true;
                    }
                    if dump {
                        println!("{:?}", func.code);
                    }
                    for op in &func.code {
                        if let vcod_gsc::bytecode::Op::CallBuiltin { name, .. } = op {
                            let n = interner.resolve(*name);
                            println!("BUILTIN\t{n}\t{rel}");
                            // Retail refuses a call that is no function at
                            // all as `unknown function`.
                            if builtins.is_some() && !is_builtin(&n.to_ascii_lowercase()) {
                                println!("UNKNOWN\t{n}\t{rel}");
                            }
                        }
                    }
                }
                fns += f.len();
                if !bad {
                    ok += 1;
                }
            }
            Err(e) => println!("FAIL\t{rel}:{}\tcompile\t{}", e.line, e.msg),
        }
    }
    println!("SUMMARY files={} ok={ok} functions={fns}", files.len());
}
