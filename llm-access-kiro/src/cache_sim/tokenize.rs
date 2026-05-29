//! Conversion of canonical input units into fixed-size canonical token pages
//! and the text-atom tokenizer.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn build_token_pages(units: &[CanonicalInputUnit]) -> Vec<CanonicalTokenPage> {
    let mut pages = Vec::new();
    let mut current = Vec::<u64>::with_capacity(PREFIX_CACHE_PAGE_SIZE);
    for atom in units
        .iter()
        .flat_map(|unit| unit.token_atoms.iter().copied())
    {
        current.push(atom);
        if current.len() == PREFIX_CACHE_PAGE_SIZE {
            pages.push(build_token_page(&current));
            current.clear();
        }
    }
    if !current.is_empty() {
        pages.push(build_token_page(&current));
    }
    pages
}
// A page key is the hash of the packed token atom stream. The tree stores only
// this compact page identity plus token count; it does not retain the original
// strings or token vectors per node.
pub(crate) fn build_token_page(atoms: &[u64]) -> CanonicalTokenPage {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(atoms));
    for atom in atoms {
        bytes.extend_from_slice(&atom.to_le_bytes());
    }
    CanonicalTokenPage {
        key: xxh3_128(&bytes),
        token_count: u16::try_from(atoms.len()).expect("page token count should fit in u16"),
    }
}
pub(crate) fn tokenize_text_atoms(text: &str) -> Vec<u64> {
    let mut atoms = Vec::new();
    let mut ascii_word_start = None::<usize>;
    let mut ascii_word_end = 0usize;

    for (index, ch) in text.char_indices() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if ascii_word_start.is_none() {
                ascii_word_start = Some(index);
            }
            ascii_word_end = index + ch.len_utf8();
            continue;
        }

        if let Some(start) = ascii_word_start.take() {
            atoms.push(hash_token_atom(&text[start..ascii_word_end]));
        }

        if ch.is_whitespace() {
            continue;
        }

        let end = index + ch.len_utf8();
        atoms.push(hash_token_atom(&text[index..end]));
    }

    if let Some(start) = ascii_word_start {
        atoms.push(hash_token_atom(&text[start..ascii_word_end]));
    }

    if atoms.is_empty() && !text.is_empty() {
        atoms.push(hash_token_atom(text));
    }
    atoms
}
pub(crate) fn hash_token_atom(text: &str) -> u64 {
    xxh3_64(text.as_bytes())
}
