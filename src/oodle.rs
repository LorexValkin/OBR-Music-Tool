//! Oodle block decompression through the pure-Rust `oozextract` (no DLL, no network).
//! Shared between the application and `tools/sfxindex`.

use crate::iostore::Decompressor;
use anyhow::{anyhow, bail, Result};

pub struct Oodle {
    extractor: oozextract::Extractor,
}

impl Oodle {
    pub fn new() -> Oodle {
        Oodle { extractor: oozextract::Extractor::new() }
    }
}

impl Default for Oodle {
    fn default() -> Self {
        Oodle::new()
    }
}

impl Decompressor for Oodle {
    fn decompress(&mut self, method: &str, input: &[u8], output: &mut [u8]) -> Result<()> {
        if !method.eq_ignore_ascii_case("oodle") {
            bail!("unsupported compression method {method:?}");
        }
        let n = self
            .extractor
            .read_from_slice(input, output)
            .map_err(|e| anyhow!("oodle: {e:?}"))?;
        if n != output.len() {
            bail!("oodle: decoded {n} bytes, expected {}", output.len());
        }
        Ok(())
    }
}
