//! Human-readable twin of the binary index, in the same order, for git diffs.

use crate::format::{RawIndex, EV_HAS_SHARED_MEDIA, EV_HAS_UNPAIRED, EV_LOCALISED, EV_PREFETCH_SUSPECT, NONE};
use std::fmt::Write;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn flag_names(flags: u8) -> String {
    let mut v = Vec::new();
    if flags & EV_PREFETCH_SUSPECT != 0 {
        v.push("prefetch");
    }
    if flags & EV_HAS_SHARED_MEDIA != 0 {
        v.push("shared");
    }
    if flags & EV_LOCALISED != 0 {
        v.push("localised");
    }
    if flags & EV_HAS_UNPAIRED != 0 {
        v.push("unpaired");
    }
    v.join(",")
}

/// `hidden` = `(path, reason)` pairs listed after the visible rows.
pub fn render(raw: &RawIndex, hidden: &[(String, String)]) -> String {
    let h = raw.header;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# sfxindex format={} rules={} utoc_fingerprint={:016x} pak_index_hash={} events={} wems={} media_refs={}",
        h.version, h.rules_version, h.utoc_fingerprint, hex(&h.pak_index_hash), h.event_count, h.wem_count, h.media_ref_count
    );
    out.push_str("tab\tgroup\tevent_path\twem_id\tsource_wav\tflags\tlength_ms\n");
    for ti in 0..h.tab_count as usize {
        let tab = raw.tab(ti);
        let tab_name = raw.string(tab.name);
        for ei in tab.first_event..tab.first_event + tab.event_count {
            let ev = raw.event(ei as usize);
            let group = raw.string(raw.group(ev.group as usize).name);
            let path = raw.string(ev.path);
            let flags = flag_names(ev.flags);
            for mi in ev.first_media..ev.first_media + ev.media_count as u32 {
                let wem = raw.wem(raw.media_ref(mi as usize) as usize);
                let wav = if wem.wav == NONE { "" } else { raw.string(wem.wav) };
                let _ = writeln!(out, "{tab_name}\t{group}\t{path}\t{}\t{wav}\t{flags}\t{}", wem.id, wem.duration_ms);
            }
        }
    }
    let mut hidden: Vec<&(String, String)> = hidden.iter().collect();
    hidden.sort();
    for (path, reason) in hidden {
        let _ = writeln!(out, "(hidden)\t{reason}\t{path}\t\t\t\t");
    }
    out
}
