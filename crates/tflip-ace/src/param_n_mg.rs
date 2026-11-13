//! The `Parameter N` of the administrator calls (`AdsMg*`) given a NULL
//! pointer, and `AdsGetHandleType` of an `AdsMgConnect` handle
//! (`fixtures/errors/param_n_mg.json`, from `tools/ace/errors/param_n_mg.c`).

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::Value as Json;

    use crate::fixtures;

    macro_rules! exports {
        ($($name:ident),* $(,)?) => {
            /// The address of the export `name`.
            fn export(name: &str) -> Option<usize> {
                match name {
                    $(stringify!($name) => Some(crate::$name as *const () as usize),)*
                    _ => None,
                }
            }
        };
    }

    exports!(
        AdsGetHandleType,
        AdsMgGetServerType,
        AdsMgGetCommStats,
        AdsMgResetCommStats,
        AdsMgGetActivityInfo,
        AdsMgGetInstallInfo,
        AdsMgGetConfigInfo,
        AdsMgGetUserNames,
        AdsMgGetOpenTables,
        AdsMgGetOpenIndexes,
        AdsMgGetLocks,
        AdsMgGetLockOwner,
        AdsMgGetWorkerThreadActivity,
        AdsMgKillUser,
        AdsMgDumpInternalTables,
        AdsMgKillUser90,
        AdsMgDisconnect,
        AdsMgConnect,
    );

    /// Calls the export at `address` with `args`, all as machine words, the
    /// way the probe does.
    fn call(address: usize, args: &[usize]) -> u32 {
        // SAFETY: each export takes as many words as the case gives it
        // (integers and pointers, which the C ABI passes alike), and
        // returns a `u32`; the pointers are the test's buffers or null.
        unsafe {
            match *args {
                [a] => std::mem::transmute::<usize, unsafe extern "system" fn(usize) -> u32>(
                    address,
                )(a),
                [a, b] => std::mem::transmute::<
                    usize,
                    unsafe extern "system" fn(usize, usize) -> u32,
                >(address)(a, b),
                [a, b, c] => std::mem::transmute::<
                    usize,
                    unsafe extern "system" fn(usize, usize, usize) -> u32,
                >(address)(a, b, c),
                [a, b, c, d] => std::mem::transmute::<
                    usize,
                    unsafe extern "system" fn(usize, usize, usize, usize) -> u32,
                >(address)(a, b, c, d),
                [a, b, c, d, e] => std::mem::transmute::<
                    usize,
                    unsafe extern "system" fn(usize, usize, usize, usize, usize) -> u32,
                >(address)(a, b, c, d, e),
                [a, b, c, d, e, f] => std::mem::transmute::<
                    usize,
                    unsafe extern "system" fn(usize, usize, usize, usize, usize, usize) -> u32,
                >(address)(a, b, c, d, e, f),
                [a, b, c, d, e, f, g] => std::mem::transmute::<
                    usize,
                    unsafe extern "system" fn(
                        usize,
                        usize,
                        usize,
                        usize,
                        usize,
                        usize,
                        usize,
                    ) -> u32,
                >(address)(a, b, c, d, e, f, g),
                _ => panic!("{} arguments", args.len()),
            }
        }
    }

    fn last_error() -> (u32, String) {
        let (mut code, mut buf, mut len) = (0, [0u8; 1200], 1200);
        unsafe { crate::AdsGetLastError(&mut code, buf.as_mut_ptr(), &mut len) };
        let text = String::from_utf8_lossy(&buf[..usize::from(len)]).into_owned();
        (code, text.trim_end_matches('\0').to_owned())
    }

    /// Whether the original's answer depends on what its server has in use
    /// (the user ADSSYS is not connected, a file is not open there), which
    /// tflip does not account for in the same way.
    fn server_state(want: (u64, u64, &str)) -> bool {
        matches!(want.0, 7050 | 7051)
    }

    /// Replays `fixtures/errors/param_n_mg.json` on `connect`ions to the
    /// fixture folder `indexes`. The probe ran each row on a management
    /// handle (`m`), a connection to the server (`c`), a local connection
    /// (`l`, skipped when the test runs against a server) and a handle that
    /// was disconnected (`x`).
    fn replay() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../");
        let rows: Vec<Json> = serde_json::from_slice(
            &std::fs::read(root.join("fixtures/errors/param_n_mg.json")).unwrap(),
        )
        .unwrap();
        let _settings = fixtures::settings_shared();
        let source = fixtures::source("indexes");
        // The original's `AdsMgConnect` takes only a server's name; tflip's
        // is `AdsConnect`'s, which needs the share too, and its local
        // engine has no server to name, so `m` is a local connection there.
        let server = match fixtures::is_remote() {
            true => source.clone(),
            false => "127.0.0.1".to_owned(),
        };
        let server = std::ffi::CString::new(server).unwrap();
        let mgmt = match fixtures::is_remote() {
            true => {
                let mut mgmt = 0;
                assert_eq!(
                    unsafe {
                        crate::AdsMgConnect(
                            server.as_ptr().cast_mut().cast(),
                            c"ADSSYS".as_ptr().cast_mut().cast(),
                            std::ptr::null_mut(),
                            &mut mgmt,
                        )
                    },
                    0
                );
                mgmt
            }
            false => fixtures::connect_to(&source),
        };
        let connect = fixtures::connect_to(&source);
        let dead = fixtures::connect_to(&source);
        crate::AdsDisconnect(dead);
        let local = match fixtures::is_remote() {
            true => 0,
            false => fixtures::connect_to(&source),
        };
        let mut differ = Vec::new();
        for row in rows {
            let line = row["call"].as_str().unwrap();
            let words: Vec<&str> = line.split(' ').collect();
            if fixtures::is_remote() && words.contains(&"l") {
                continue;
            }
            let null = usize::try_from(row["null"].as_u64().unwrap()).unwrap();
            let mut buf = vec![0u8; 100_000];
            let mut n16 = [0u16; 8];
            let mut n32 = [0u32; 8];
            let mut connected = 0usize;
            let strings: Vec<std::ffi::CString> = words
                .iter()
                .map(|word| {
                    let text = word.strip_prefix("str:").or(word.strip_prefix("pstr:"));
                    std::ffi::CString::new(text.unwrap_or("")).unwrap()
                })
                .collect();
            let mut args = Vec::new();
            for (i, word) in words.iter().enumerate().skip(1) {
                let value = match *word {
                    "m" => mgmt,
                    "pstr:127.0.0.1" if words[0] == "AdsMgConnect" => server.as_ptr() as usize,
                    "c" => connect,
                    "l" => local,
                    "x" => dead,
                    "pbuf" => buf.as_mut_ptr() as usize,
                    "p16" => &mut n16[i] as *mut u16 as usize,
                    "p32" if words[0] == "AdsMgConnect" => &mut connected as *mut usize as usize,
                    "p32" => &mut n32[i] as *mut u32 as usize,
                    word if word.starts_with("pl16:") => {
                        n16[i] = word[5..].parse().unwrap();
                        &mut n16[i] as *mut u16 as usize
                    }
                    word if word.starts_with("pl32:") => {
                        n32[i] = word[5..].parse().unwrap();
                        &mut n32[i] as *mut u32 as usize
                    }
                    word if word.starts_with("str:") || word.starts_with("pstr:") => {
                        strings[i].as_ptr() as usize
                    }
                    number => number.parse().unwrap(),
                };
                args.push(if i == null { 0 } else { value });
            }
            // A failure of another code, so that a call that leaves the
            // last error alone shows it.
            unsafe { crate::AdsSetDateFormat(std::ptr::null_mut()) };
            let address = export(words[0]).unwrap_or_else(|| panic!("{}", words[0]));
            let rc = call(address, &args);
            if connected != 0 {
                crate::AdsDisconnect(connected);
            }
            let (code, text) = last_error();
            let want = (
                row["rc"].as_u64().unwrap(),
                row["last"].as_u64().unwrap(),
                row["text"].as_str().unwrap(),
            );
            // The rows the server's own state shapes are not matched (see
            // KNOWN_ISSUES).
            // Without a server there is nothing to connect to.
            let connecting = words[0] == "AdsMgConnect" && want.0 == 0 && !fixtures::is_remote();
            if connecting
                || server_state(want)
                || (words[0] == "AdsMgKillUser" && null == 0 && words[1] != "l")
            {
                continue;
            }
            let mut got = vec![u64::from(rc), u64::from(code)];
            let mut expected = vec![want.0, want.1];
            if let Some(kind) = row["type"]
                .as_u64()
                .filter(|_| words[0] == "AdsGetHandleType")
            {
                got.push(u64::from(n16[2]));
                expected.push(kind);
            }
            if got != expected || text != want.2 {
                differ.push(format!(
                    "{line} (null {null}):\n    got  {got:?} {text:?}\n    want {expected:?} {:?}",
                    want.2
                ));
            }
        }
        crate::AdsDisconnect(connect);
        crate::AdsDisconnect(mgmt);
        if local != 0 {
            crate::AdsDisconnect(local);
        }
        assert!(
            differ.is_empty(),
            "{} differ:\n{}",
            differ.len(),
            differ.join("\n")
        );
    }

    #[test]
    fn administrator_calls_report_null_pointers_as_the_original_does() {
        replay();
    }

    #[test]
    fn administrator_calls_report_null_pointers_as_the_original_does_remotely() {
        fixtures::remotely(replay);
    }
}
