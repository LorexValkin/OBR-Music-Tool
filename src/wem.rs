use anyhow::{Context, Result};
use std::path::Path;

use crate::wem_encoder;

pub fn convert_to_wem(source: &Path, output: &Path) -> Result<()> {
    wem_encoder::encode_to_wem(source, output)
        .with_context(|| format!("converting {} to WEM", source.display()))
}
