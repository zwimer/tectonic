// Copyright 2021 the Tectonic Project
// Licensed under the MIT License.

//! Decode a format file.

use nom::{
    multi::count,
    number::complete::{be_i16, be_i32, be_i64, be_u16},
    Err as NomErr, IResult,
};
use std::io::Write;
use tectonic_errors::prelude::*;

use crate::{
    base::{MAX_HALFWORD, MIN_HALFWORD},
    catcodes::CatCode,
    cshash,
    engine::Engine,
    eqtb, mem, parseutils, stringtable, FormatVersion,
};

/// Saved Tectonic/XeTeX engine state, decoded into memory.
///
/// This public API of this structure isn't yet complete. It parses format files
/// but does not yet provide proper runtime introspection of the results.
#[derive(Debug)]
#[allow(dead_code)] // TEMPORARY!
pub struct Format {
    engine: Engine,
    strings: stringtable::StringTable,
    mem: mem::Memory,
    eqtb: eqtb::EquivalenciesTable,
    cshash: cshash::ControlSeqHash,
}

// Parsing

const HEADER_MAGIC: i32 = 0x54_54_4E_43; // ASCII "TTNC"
const FOOTER_MAGIC: i32 = 0x00_00_02_9A;
const TOO_BIG_CHAR: i32 = 0x0001_0000;

const HYPH_SIZE: usize = 8191;

const TRIE_OP_SIZE: i32 = 35111;

const BIGGEST_LANG: usize = 255;

const MAX_USV: i32 = crate::base::NUMBER_USVS as i32;

impl Format {
    pub fn parse(input: &[u8]) -> Result<Self> {
        match parse_impl(input) {
            Ok((_remainder, result)) => Ok(result),
            Err(NomErr::Error(inner)) => bail!("parse error: {}", inner.code.description()),
            Err(NomErr::Failure(inner)) => bail!("parse failure: {}", inner.code.description()),
            Err(NomErr::Incomplete(_)) => bail!("incomplete input"),
        }
    }

    pub fn dump_string_table<W: Write>(&self, stream: &mut W) -> Result<()> {
        for sp in self.strings.all_sps() {
            let value = self.strings.lookup(sp);
            writeln!(stream, "{} = \"{}\"", sp, value)?;
        }

        Ok(())
    }

    pub fn dump_actives<W: Write>(&self, stream: &mut W) -> Result<()> {
        for chr in 0..MAX_USV {
            let entry = self.eqtb_active(chr);

            if entry.ty == self.engine.settings.undefined_cs_command {
                continue;
            }

            let cur_cat = self.eqtb_catcode(chr)?;

            writeln!(
                stream,
                "{} ({}) => {:?}",
                fmt_usv(chr),
                cur_cat.abbrev(),
                entry
            )?;
        }

        Ok(())
    }

    pub fn dump_catcodes<W: Write>(&self, stream: &mut W) -> Result<()> {
        let mut blocks = vec![Vec::new(); 16];
        let mut start = 0;
        let mut prev = start;
        let mut cur_cat = self.eqtb_catcode(start)?;

        for chr in 1..MAX_USV {
            let cat = self.eqtb_catcode(chr)?;

            if cat != cur_cat {
                blocks[cur_cat as usize].push((start, prev));
                start = chr;
                cur_cat = cat;
            }

            prev = chr;
        }

        blocks[cur_cat as usize].push((start, prev));

        for cat in 0..16 {
            if cat > 0 {
                writeln!(stream)?;
            }

            writeln!(stream, "{}:", CatCode::from_i32(cat).unwrap().description())?;

            for block in &blocks[cat as usize] {
                let (start, end) = *block;

                if end == start {
                    writeln!(stream, "    {}", fmt_usv(start))?;
                } else {
                    writeln!(stream, "    {} - {}", fmt_usv(start), fmt_usv(end))?;
                }
            }
        }

        Ok(())
    }

    // Decoding various eqtb bits. These could just as well be methods on the Eqtb
    // type, except it doesn't actually hold onto all of the magic offsets needed
    // to index into it properly.

    fn eqtb_active(&self, c: i32) -> eqtb::EqtbEntry {
        assert!(c >= 0 && c < MAX_USV);
        self.eqtb.decode(self.engine.settings.active_base + c)
    }

    fn eqtb_catcode(&self, c: i32) -> Result<CatCode> {
        assert!(c >= 0 && c < MAX_USV);
        CatCode::from_i32(
            self.eqtb
                .decode(self.engine.settings.cat_code_base + c)
                .value,
        )
    }
}

fn parse_impl(input: &[u8]) -> IResult<&[u8], Format> {
    let (input, _) = parseutils::satisfy_be_i32(HEADER_MAGIC)(input)?;
    let (input, serial) = be_i32(input)?;
    let engine = Engine::new_for_version(serial as FormatVersion);

    let (input, hash_high) = be_i32(input)?;
    let (input, _mem_top) = parseutils::satisfy_be_i32(engine.settings.mem_top)(input)?;
    let (input, _eqtb_size) = parseutils::satisfy_be_i32(engine.settings.eqtb_size)(input)?;
    let (input, _hash_prime) =
        parseutils::satisfy_be_i32(engine.settings.hash_prime as i32)(input)?;
    let (input, _hyph_prime) = be_i32(input)?;

    // string table

    let (input, strings) = stringtable::StringTable::parse(input)?;

    // "mem" array

    let (input, mem) = mem::Memory::parse(input, &engine)?;

    // eqtb

    let (input, eqtb) = eqtb::EquivalenciesTable::parse(input, &engine, hash_high)?;

    // nominally hash_top, but hash_top = engine.settings.eqtb_top since hash_extra is nonzero
    let (input, _par_loc) = parseutils::ranged_be_i32(
        engine.settings.hash_base as i32,
        engine.settings.eqtb_top as i32,
    )(input)?;

    let (input, _write_loc) = parseutils::ranged_be_i32(
        engine.settings.hash_base as i32,
        engine.settings.eqtb_top as i32,
    )(input)?;

    // Primitives. TODO: figure out best type for `prims`.

    let (input, _prims) = count(be_i64, engine.settings.prim_size as usize + 1)(input)?;

    // Control sequence names -- the hash table.

    let (input, cshash) = cshash::ControlSeqHash::parse(input, &engine, hash_high)?;

    // font info

    let (input, fmem_ptr) = parseutils::ranged_be_i32(7, 147483647)(input)?;

    let (input, _font_info) = count(be_i64, fmem_ptr as usize)(input)?;

    // NB: FONT_BASE = 0
    let (input, font_ptr) = parseutils::ranged_be_i32(0, engine.settings.max_fonts as i32)(input)?;

    let n_fonts = font_ptr as usize + 1;
    let (input, _font_check) = count(be_i64, n_fonts)(input)?;
    let (input, _font_size) = count(be_i32, n_fonts)(input)?;
    let (input, _font_dsize) = count(be_i32, n_fonts)(input)?;
    let (input, _font_params) = count(
        parseutils::ranged_be_i32(MIN_HALFWORD, MAX_HALFWORD),
        n_fonts,
    )(input)?;
    let (input, _hyphen_char) = count(be_i32, n_fonts)(input)?;
    let (input, _skew_char) = count(be_i32, n_fonts)(input)?;
    let (input, _font_name) = count(be_i32, n_fonts)(input)?;
    let (input, _font_area) = count(be_i32, n_fonts)(input)?;
    let (input, _font_bc) = count(be_i16, n_fonts)(input)?;
    let (input, _font_ec) = count(be_i16, n_fonts)(input)?;
    let (input, _char_base) = count(be_i32, n_fonts)(input)?;
    let (input, _width_base) = count(be_i32, n_fonts)(input)?;
    let (input, _height_base) = count(be_i32, n_fonts)(input)?;
    let (input, _depth_base) = count(be_i32, n_fonts)(input)?;
    let (input, _italic_base) = count(be_i32, n_fonts)(input)?;
    let (input, _lig_kern_base) = count(be_i32, n_fonts)(input)?;
    let (input, _kern_base) = count(be_i32, n_fonts)(input)?;
    let (input, _exten_base) = count(be_i32, n_fonts)(input)?;
    let (input, _param_base) = count(be_i32, n_fonts)(input)?;
    let (input, _font_glue) = count(
        parseutils::ranged_be_i32(MIN_HALFWORD, mem.lo_mem_max),
        n_fonts,
    )(input)?;
    let (input, _bchar_label) = count(parseutils::ranged_be_i32(0, fmem_ptr - 1), n_fonts)(input)?;
    let (input, _font_bchar) = count(parseutils::ranged_be_i32(0, TOO_BIG_CHAR), n_fonts)(input)?;
    let (input, _font_false_bchar) =
        count(parseutils::ranged_be_i32(0, TOO_BIG_CHAR), n_fonts)(input)?;

    // Hyphenations!

    let (input, hyph_count) = be_i32(input)?;

    let (input, mut _hyph_next) = be_i32(input)?;

    let mut hyph_next;
    let mut hyph_link = vec![0u16; HYPH_SIZE];
    let mut hyph_word = vec![0i32; HYPH_SIZE];
    let mut hyph_list = vec![0i32; HYPH_SIZE];
    let mut input = input;
    let max_word = strings.len() as i32 + TOO_BIG_CHAR - 1;

    for _ in 0..hyph_count {
        let (ii, mut j) = be_i32(input)?;

        if j > 0xFFFF {
            hyph_next = j / 0x10000;
            j -= hyph_next * 0x10000;
        } else {
            hyph_next = 0;
        }

        hyph_link[j as usize] = hyph_next as u16;

        let (ii, w) = parseutils::ranged_be_i32(0, max_word)(ii)?;
        hyph_word[j as usize] = w;

        let (ii, l) = parseutils::ranged_be_i32(MIN_HALFWORD, MAX_HALFWORD)(ii)?;
        hyph_list[j as usize] = l;

        input = ii;
    }

    // trie

    let (input, trie_max) = be_i32(input)?;

    let (input, _hyph_start) = parseutils::ranged_be_i32(0, trie_max)(input)?;

    let n_trie = trie_max as usize + 1;
    let (input, _trie_trl) = count(be_i32, n_trie)(input)?;
    let (input, _trie_tro) = count(be_i32, n_trie)(input)?;
    let (input, _trie_trc) = count(be_u16, n_trie)(input)?;

    let (input, _max_hyph_char) = be_i32(input)?;

    let (input, trie_op_ptr) = parseutils::ranged_be_i32(0, TRIE_OP_SIZE)(input)?;

    // IMPORTANT!!! XeTeX loads these into 1-based indices!
    let (input, _hyf_distance) = count(be_i16, trie_op_ptr as usize)(input)?;
    let (input, _hyf_num) = count(be_i16, trie_op_ptr as usize)(input)?;
    let (input, _hyf_next) = count(be_u16, trie_op_ptr as usize)(input)?;

    let mut trie_used = vec![0i32; BIGGEST_LANG + 1];
    let mut op_start = vec![0i32; BIGGEST_LANG + 1];

    let mut k = BIGGEST_LANG + 1;
    let mut j = trie_op_ptr;
    let mut input = input;

    while j > 0 {
        let (ii, new_k) = parseutils::ranged_be_i32(0, k as i32 - 1)(input)?;
        k = new_k as usize;
        let (ii, u) = parseutils::ranged_be_i32(1, j)(ii)?;
        trie_used[k] = u;
        j -= u;
        op_start[k] = j;
        input = ii;
    }

    // All done!

    let (input, _) = parseutils::satisfy_be_i32(FOOTER_MAGIC)(input)?;

    let fmt = Format {
        engine,
        strings,
        mem,
        eqtb,
        cshash,
    };
    Ok((input, fmt))
}

fn fmt_usv(c: i32) -> String {
    // Valid inputs are valid USVs, which are as per the Unicode Glossary: "Any
    // Unicode code point except high-surrogate and low-surrogate code points.
    // In other words, the ranges of integers 0x0 to 0xD7FF and 0xE000 to
    // 0x10FFFF, inclusive."
    let maybe_chr = char::from_u32(c as u32);

    if let Some(chr) = maybe_chr {
        if chr == ' ' {
            format!("' ' (0x{:06x})", c)
        } else if chr == '\'' {
            format!("\\' (0x{:06x})", c)
        } else if chr == '\"' {
            format!("\\\" (0x{:06x})", c)
        } else if chr.is_control() || chr.is_whitespace() {
            format!("{} (0x{:06x})", chr.escape_default(), c)
        } else {
            format!("{} (0x{:06x})", chr, c)
        }
    } else {
        format!("*invalid* (0x{:06x})", c)
    }
}
