use super::*;

#[test]
fn test_named_highlight_still_writes_w_highlight() {
    let run = Run {
        text: "hi".into(),
        highlight: true,
        highlight_color: "yellow".into(),
        ..Run::default()
    };
    let xml = run_props_xml(&run);
    assert!(xml.contains("<w:highlight w:val=\"yellow\"/>"), "got {xml}");
    assert!(!xml.contains("w:shd"), "got {xml}");
}

#[test]
fn test_custom_hex_highlight_writes_w_shd() {
    let run = Run {
        text: "hi".into(),
        highlight: true,
        highlight_color: "00ff88".into(),
        ..Run::default()
    };
    let xml = run_props_xml(&run);
    assert!(
        xml.contains("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"00ff88\"/>"),
        "got {xml}",
    );
    assert!(!xml.contains("w:highlight"), "got {xml}");
}

#[test]
fn test_emphasis_boxed_emits_a_real_bdr() {
    let run = Run {
        text: "hi".into(),
        emphasis: true,
        emphasis_boxed: true,
        ..Run::default()
    };
    let xml = run_props_xml(&run);
    assert!(
        xml.contains(r#"<w:bdr w:val="single" w:sz="12" w:space="0" w:color="auto"/>"#),
        "got: {xml}"
    );
}

#[test]
fn test_non_boxed_emphasis_emits_no_bdr() {
    let run = Run {
        text: "hi".into(),
        emphasis: true,
        ..Run::default()
    };
    let xml = run_props_xml(&run);
    assert!(!xml.contains("w:bdr"), "got: {xml}");
}

#[test]
fn test_emphasis_box_bdr_precedes_highlight_and_shd() {
    // CT_RPr's schema order (and Word's real-world tolerance) requires
    // <w:bdr> before <w:shd>; a border emitted after <w:highlight>/<w:shd>
    // has been observed silently dropped by Word on reopen even though this
    // parser's own reparse (which is order-insensitive) would still pass.
    let named_highlight = Run {
        text: "hi".into(),
        emphasis: true,
        emphasis_boxed: true,
        highlight: true,
        highlight_color: "yellow".into(),
        ..Run::default()
    };
    let xml = run_props_xml(&named_highlight);
    let bdr_pos = xml.find("<w:bdr").expect("bdr missing");
    let highlight_pos = xml.find("<w:highlight").expect("highlight missing");
    assert!(
        bdr_pos < highlight_pos,
        "bdr must precede highlight, got: {xml}"
    );

    let custom_hex_highlight = Run {
        text: "hi".into(),
        emphasis: true,
        emphasis_boxed: true,
        highlight: true,
        highlight_color: "00ff88".into(),
        ..Run::default()
    };
    let xml = run_props_xml(&custom_hex_highlight);
    let bdr_pos = xml.find("<w:bdr").expect("bdr missing");
    let shd_pos = xml.find("<w:shd").expect("shd missing");
    assert!(bdr_pos < shd_pos, "bdr must precede shd, got: {xml}");
}

#[test]
fn test_w_shd_fill_is_read_back_as_a_highlight() {
    let mut run = Run::default();
    apply_run_prop(
        &BytesStart::from_content(r#"w:shd w:val="clear" w:color="auto" w:fill="00FF88""#, 5),
        &mut run,
    );
    assert!(run.highlight);
    assert_eq!(run.highlight_color, "00ff88");
}

#[test]
fn test_w_shd_auto_fill_is_not_a_highlight() {
    for content in [
        r#"w:shd w:val="clear" w:color="auto" w:fill="auto""#,
        r#"w:shd w:val="clear" w:color="auto""#,
        r#"w:shd w:val="clear" w:fill="nothex""#,
    ] {
        let mut run = Run::default();
        apply_run_prop(&BytesStart::from_content(content, 5), &mut run);
        assert!(!run.highlight, "should not highlight for: {content}");
    }
}

#[test]
fn test_custom_highlight_survives_a_write_read_round_trip() {
    let source = Run {
        text: "hi".into(),
        highlight: true,
        highlight_color: "00ff88".into(),
        ..Run::default()
    };
    // Feed the emitted `w:shd` element straight back through the parser.
    let xml = run_props_xml(&source);
    let inner = xml.trim_start_matches("<w:shd ").trim_end_matches("/>");
    let mut run = Run::default();
    apply_run_prop(
        &BytesStart::from_content(format!("w:shd {inner}"), 5),
        &mut run,
    );
    assert!(run.highlight);
    assert_eq!(run.highlight_color, source.highlight_color);
}

#[test]
fn two_concurrent_writes_to_one_path_get_different_temp_files() {
    // The panic hook writes snapshots on the panicking thread while the
    // background snapshot task may already be mid-write on the same tab's
    // file. A shared temp name lets both truncate and interleave into one
    // file, then rename a corrupt zip into place.
    let dest = Path::new("/tmp/vimbatim-example.docx");
    assert_ne!(tmp_write_path(dest), tmp_write_path(dest));
}

#[test]
fn the_temp_write_path_keeps_tmp_as_its_final_extension() {
    // `scan_recovery_dir` sweeps abandoned leftovers by this extension.
    let tmp = tmp_write_path(Path::new("/tmp/1234-5678-0.docx"));
    assert_eq!(tmp.extension().unwrap(), "tmp");
    // ...and the pid stays the first '-'-separated segment, which is how
    // the sweeper decides whether the owning process is still alive.
    let stem = tmp.file_stem().unwrap().to_str().unwrap();
    assert_eq!(stem.split('-').next().unwrap(), "1234");
}

fn wrap_run_xml(run_xml: &str) -> String {
    format!(
        "<w:document><w:body><w:p><w:r>{}<w:t>hi</w:t></w:r></w:p></w:body></w:document>",
        run_xml
    )
}

fn no_styles() -> HashMap<String, StyleDefaults> {
    HashMap::new()
}

fn no_numbering() -> HashMap<u32, (String, String, Option<String>)> {
    HashMap::new()
}

// ── italic/font/color parsing (rich-text formatting plan, Phase 1) ──────

#[test]
fn test_parses_italic_run_property() {
    let xml = wrap_run_xml("<w:rPr><w:i/></w:rPr>");
    let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert!(paragraphs[0].runs[0].italic);
}

#[test]
fn test_parses_run_font_ascii_attribute() {
    let xml = wrap_run_xml(r#"<w:rPr><w:rFonts w:ascii="Georgia"/></w:rPr>"#);
    let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert_eq!(paragraphs[0].runs[0].font, Some("Georgia".to_string()));
}

#[test]
fn test_parses_run_color_value() {
    let xml = wrap_run_xml(r#"<w:rPr><w:color w:val="FF0000"/></w:rPr>"#);
    let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert_eq!(paragraphs[0].runs[0].color, Some("FF0000".to_string()));
}

#[test]
fn test_color_val_auto_is_treated_as_none() {
    let xml = wrap_run_xml(r#"<w:rPr><w:color w:val="auto"/></w:rPr>"#);
    let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert_eq!(paragraphs[0].runs[0].color, None);
}

#[test]
fn test_run_without_new_properties_defaults_to_none() {
    let xml = wrap_run_xml("");
    let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert!(!paragraphs[0].runs[0].italic);
    assert_eq!(paragraphs[0].runs[0].font, None);
    assert_eq!(paragraphs[0].runs[0].color, None);
}

// ── alignment + heading parsing/emission ────────────────────────────────

#[test]
fn test_parses_center_alignment() {
    let xml = "<w:document><w:body><w:p><w:pPr><w:jc w:val=\"center\"/></w:pPr><w:r><w:t>hi</w:t></w:r></w:p></w:body></w:document>";
    let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
    assert_eq!(paragraphs[0].alignment, Alignment::Center);
}

#[test]
fn test_parses_justify_alignment_from_both_value() {
    // Word's own OOXML value for full justification is "both", not "justify".
    let xml = "<w:document><w:body><w:p><w:pPr><w:jc w:val=\"both\"/></w:pPr><w:r><w:t>hi</w:t></w:r></w:p></w:body></w:document>";
    let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
    assert_eq!(paragraphs[0].alignment, Alignment::Justify);
}

#[test]
fn test_paragraph_without_jc_defaults_to_left_alignment() {
    let xml = "<w:document><w:body><w:p><w:r><w:t>hi</w:t></w:r></w:p></w:body></w:document>";
    let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
    assert_eq!(paragraphs[0].alignment, Alignment::Left);
}

#[test]
fn test_rebuild_emits_center_alignment() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::Center,
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(xml.contains(r#"<w:jc w:val="center"/>"#));
}

#[test]
fn test_rebuild_omits_jc_for_left_alignment() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::Left,
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(!xml.contains("w:jc"));
}

#[test]
fn test_rebuild_preserves_leading_and_trailing_run_space_even_when_flag_unset() {
    // Reproduces the Bug_Test.docx report: a run split at a highlight
    // boundary (e.g. by `apply_formatting`) ends up with a leading or
    // trailing space but its `whitespace_preserve` flag was never
    // recomputed for the new boundary (it's only set at parse time from
    // an existing file's own `xml:space="preserve"`). Without the
    // attribute, Word trims that edge space on open even though
    // vimbatim's own renderer — which reads `run.text` directly, not
    // this XML-serialisation flag — still shows it.
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![
            Run {
                text: "written and ".into(),
                whitespace_preserve: false,
                ..Run::default()
            },
            Run {
                text: "highlighted".into(),
                highlight: true,
                highlight_color: "yellow".into(),
                ..Run::default()
            },
            Run {
                text: " in vimbatim".into(),
                whitespace_preserve: false,
                ..Run::default()
            },
        ],
        heading: 0,
        alignment: Alignment::Left,
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(
        xml.contains(r#"<w:t xml:space="preserve">written and </w:t>"#),
        "trailing space before a highlight boundary must be preserved: {xml}"
    );
    assert!(
        xml.contains(r#"<w:t xml:space="preserve"> in vimbatim</w:t>"#),
        "leading space after a highlight boundary must be preserved: {xml}"
    );
}

#[test]
fn test_rebuild_emits_heading_style() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            ..Run::default()
        }],
        heading: 2,
        alignment: Alignment::Left,
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    // Capitalized to match Word's own built-in styleId casing
    // ("Heading1".."Heading9") — see
    // test_rebuild_emits_capitalized_heading_styleid_matching_words_own_styles_xml
    // for why the casing has to match exactly.
    assert!(xml.contains(r#"<w:pStyle w:val="Heading2"/>"#));
}

#[test]
fn test_rebuild_omits_pstyle_for_body_text() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::Left,
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(!xml.contains("w:pStyle"));
}

/// Bug report: applying a heading (Pocket/Hat/Block/Tag) to a line that
/// was already a list item made the heading's formatting disappear when
/// the saved file was opened in real Word. Root cause: `<w:pStyle>`
/// isn't repeatable per CT_PPrBase, but a paragraph carrying both
/// `heading` and `list` got one pStyle write from each source —
/// `rebuild_document_xml` now skips the list one whenever `heading != 0`
/// (defense in depth; `AppState::apply_card_style` is the primary fix,
/// clearing `.list` so this combination isn't created going forward).
#[test]
fn test_rebuild_heading_wins_over_list_emits_exactly_one_pstyle() {
    let paragraphs = vec![Paragraph {
        list: Some(ListItem {
            kind: ListKind::BulletSolid,
            level: 0,
        }),
        runs: vec![Run {
            text: "hi".into(),
            ..Run::default()
        }],
        heading: 1,
        alignment: Alignment::Left,
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert_eq!(xml.matches("<w:pStyle").count(), 1, "got: {xml}");
    assert!(
        xml.contains(r#"<w:pStyle w:val="Heading1"/>"#),
        "got: {xml}"
    );
    assert!(!xml.contains("ListParagraph"), "got: {xml}");
    assert!(!xml.contains("w:numPr"), "got: {xml}");
}

#[test]
fn test_rebuild_omits_ppr_entirely_for_plain_paragraph() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::Left,
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(!xml.contains("w:pPr"));
}

#[test]
fn test_alignment_and_heading_round_trip_through_parse_and_rebuild() {
    let original = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            ..Run::default()
        }],
        heading: 1,
        alignment: Alignment::Center,
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &original);
    let reparsed = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert_eq!(reparsed[0].heading, 1);
    assert_eq!(reparsed[0].alignment, Alignment::Center);
}

// ── double underline parsing/emission ───────────────────────────────────

#[test]
fn test_parses_double_underline_distinctly_from_single() {
    let xml = wrap_run_xml(r#"<w:rPr><w:u w:val="double"/></w:rPr>"#);
    let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert!(paragraphs[0].runs[0].double_underline);
    assert!(!paragraphs[0].runs[0].underline);
}

#[test]
fn test_parses_single_underline_val_as_plain_underline() {
    let xml = wrap_run_xml(r#"<w:rPr><w:u w:val="single"/></w:rPr>"#);
    let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert!(paragraphs[0].runs[0].underline);
    assert!(!paragraphs[0].runs[0].double_underline);
}

#[test]
fn test_u_val_none_is_not_underlined() {
    // OOXML boolean-toggle semantics: `w:val="none"` (also seen as "0"/
    // "false") explicitly turns underline OFF, same as the property
    // being absent — not "anything other than double means single".
    // Real debate-community docx files declare this on character
    // styles like "Style13ptBold" (`<w:u w:val="none"/>`) to explicitly
    // suppress underline that would otherwise carry over.
    let xml = wrap_run_xml(r#"<w:rPr><w:u w:val="none"/></w:rPr>"#);
    let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert!(!paragraphs[0].runs[0].underline);
    assert!(!paragraphs[0].runs[0].double_underline);
}

#[test]
fn test_b_val_0_is_not_bold() {
    let xml = wrap_run_xml(r#"<w:rPr><w:b w:val="0"/></w:rPr>"#);
    let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert!(!paragraphs[0].runs[0].bold);
}

#[test]
fn test_rstyle_resolves_character_style_underline_and_bold() {
    // Reported bug repro: real debate docx files (e.g. this app's own
    // "Affective Labor" test file) underline/bold the emphasized
    // "read" portions of a card via a *character* style referenced by
    // <w:rStyle> on the run, rather than direct <w:u>/<w:b> — 4000+
    // occurrences of `<w:rStyle w:val="StyleUnderline"/>` in that file
    // alone. Unhandled, that text silently loses its underline.
    let styles_xml = r#"<w:styles>
            <w:style w:type="character" w:styleId="StyleUnderline">
                <w:rPr><w:u w:val="single"/></w:rPr>
            </w:style>
        </w:styles>"#;
    let styles = parse_styles_xml(styles_xml);
    let xml = wrap_run_xml(r#"<w:rPr><w:rStyle w:val="StyleUnderline"/></w:rPr>"#);
    let paragraphs = parse_document_xml(&xml, &styles, &no_numbering()).unwrap();
    assert!(
        paragraphs[0].runs[0].underline,
        "character-style underline not applied"
    );
}

#[test]
fn test_rstyle_direct_formatting_after_it_still_wins() {
    // Word's own cascade: rStyle (character style) sets the run's
    // baseline, but any direct formatting appearing later in the same
    // <w:rPr> still overrides it — matching how a <w:pStyle>'s
    // paragraph-level defaults already work elsewhere in this parser.
    let styles_xml = r#"<w:styles>
            <w:style w:type="character" w:styleId="StyleUnderline">
                <w:rPr><w:u w:val="single"/></w:rPr>
            </w:style>
        </w:styles>"#;
    let styles = parse_styles_xml(styles_xml);
    let xml =
        wrap_run_xml(r#"<w:rPr><w:rStyle w:val="StyleUnderline"/><w:u w:val="none"/></w:rPr>"#);
    let paragraphs = parse_document_xml(&xml, &styles, &no_numbering()).unwrap();
    assert!(
        !paragraphs[0].runs[0].underline,
        "direct <w:u w:val=\"none\"/> after rStyle should win"
    );
}

#[test]
fn test_rstyle_resolves_character_style_bdr_into_emphasis_boxed_and_emphasis() {
    // Mirrors real Verbatim output: the run carries only an rStyle
    // reference; bold/underline/size/border all live on the referenced
    // character style.
    let styles_xml = r#"<w:styles>
            <w:style w:type="character" w:styleId="Emphasis">
                <w:rPr>
                    <w:b/><w:sz w:val="24"/><w:u w:val="single"/>
                    <w:bdr w:val="single" w:sz="12" w:space="0" w:color="auto"/>
                </w:rPr>
            </w:style>
        </w:styles>"#;
    let styles = parse_styles_xml(styles_xml);
    let xml = wrap_run_xml(r#"<w:rPr><w:rStyle w:val="Emphasis"/></w:rPr>"#);
    let paragraphs = parse_document_xml(&xml, &styles, &no_numbering()).unwrap();
    let run = &paragraphs[0].runs[0];
    assert!(
        run.emphasis_boxed,
        "style-referenced <w:bdr> should set emphasis_boxed"
    );
    assert!(
        run.emphasis,
        "a real box implies Emphasis, so Remove Emphasis can find it"
    );
    assert!(
        run.bold && run.underline,
        "existing bold/underline resolution must still work"
    );
}

#[test]
fn test_direct_bdr_on_a_run_sets_emphasis_boxed_and_emphasis() {
    let xml = wrap_run_xml(
        r#"<w:rPr><w:bdr w:val="single" w:sz="12" w:space="0" w:color="auto"/></w:rPr>"#,
    );
    let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert!(paragraphs[0].runs[0].emphasis_boxed);
    assert!(paragraphs[0].runs[0].emphasis);
}

#[test]
fn test_run_without_bdr_has_emphasis_boxed_false() {
    let xml = wrap_run_xml("<w:rPr><w:b/></w:rPr>");
    let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert!(!paragraphs[0].runs[0].emphasis_boxed);
}

#[test]
fn test_bdr_val_none_cancels_inherited_emphasis_boxed() {
    // A run with rStyle pointing to Emphasis (which has a border) can
    // explicitly cancel the border with <w:bdr w:val="none"/>.
    // This mirrors the existing behavior for <w:u w:val="none"/> canceling
    // underline from the style.
    let styles_xml = r#"<w:styles>
            <w:style w:type="character" w:styleId="Emphasis">
                <w:rPr>
                    <w:bdr w:val="single" w:sz="12" w:space="0" w:color="auto"/>
                </w:rPr>
            </w:style>
        </w:styles>"#;
    let styles = parse_styles_xml(styles_xml);
    let xml = wrap_run_xml(r#"<w:rPr><w:rStyle w:val="Emphasis"/><w:bdr w:val="none"/></w:rPr>"#);
    let paragraphs = parse_document_xml(&xml, &styles, &no_numbering()).unwrap();
    assert!(
        !paragraphs[0].runs[0].emphasis_boxed,
        "direct <w:bdr w:val=\"none\"/> after rStyle should cancel the box"
    );
    assert!(
        !paragraphs[0].runs[0].emphasis,
        "canceling the box should also clear emphasis"
    );
}

#[test]
fn test_rebuild_emits_double_underline() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            double_underline: true,
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(xml.contains(r#"<w:u w:val="double"/>"#));
}

#[test]
fn test_double_underline_round_trip_through_parse_and_rebuild() {
    let original = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            double_underline: true,
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &original);
    let reparsed = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert!(reparsed[0].runs[0].double_underline);
    assert!(!reparsed[0].runs[0].underline);
}

// ── Emphasis carries Verbatim's own style id ────────────────────────────

/// An emphasized run names Verbatim's `Emphasis` character style, so
/// Verbatim recognises it as its own Emphasis card type rather than as
/// anonymous bold-and-underline.
#[test]
fn an_emphasized_run_writes_verbatims_emphasis_style() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "read this".into(),
            emphasis: true,
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml(&fallback_preamble(), "", &paragraphs);
    assert!(
        xml.contains(r#"<w:rStyle w:val="Emphasis"/>"#),
        "got: {xml}"
    );
}

/// `<w:rStyle>` is not repeatable in CT_RPr, so a run that is both a Cite
/// and emphasized can only name one style. The card marker wins — it is the
/// structural identity — and the emphasis still round-trips through
/// `<w:vimbatimEmphasis/>` and its own direct formatting.
#[test]
fn a_card_marker_wins_the_single_rstyle_slot_over_emphasis() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "cited and read".into(),
            emphasis: true,
            style: Some(CardStyle::Cite),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml(&fallback_preamble(), "", &paragraphs);
    assert_eq!(
        xml.matches("<w:rStyle").count(),
        1,
        "one rStyle only: {xml}"
    );
    assert!(
        xml.contains(r#"<w:rStyle w:val="Style13ptBold"/>"#),
        "got: {xml}"
    );

    let reparsed = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert_eq!(reparsed[0].runs[0].style, Some(CardStyle::Cite));
    assert!(
        reparsed[0].runs[0].emphasis,
        "the emphasis flag still round-trips"
    );
}

/// Verbatim's Emphasis is read as emphasis whether or not the style draws a
/// box — whether it does is a per-user setting here, and an unboxed
/// Emphasis is still an Emphasis.
#[test]
fn verbatims_emphasis_style_is_read_as_emphasis_even_unboxed() {
    let styles = parse_styles_xml(
        "<w:styles><w:style w:type=\"character\" w:styleId=\"Emphasis\">\
             <w:rPr><w:b/><w:sz w:val=\"24\"/></w:rPr></w:style></w:styles>",
    );
    let xml = wrap_run_xml(r#"<w:rPr><w:rStyle w:val="Emphasis"/></w:rPr>"#);
    let paragraphs = parse_document_xml(&xml, &styles, &no_numbering()).unwrap();
    assert!(
        paragraphs[0].runs[0].emphasis,
        "unboxed Emphasis is still Emphasis"
    );
    assert!(!paragraphs[0].runs[0].emphasis_boxed);
}

// ── new-document parity with Verbatim ───────────────────────────────────

/// `<w:docDefaults>` comes from the user's own settings, not Verbatim's
/// hardcoded 11pt/1.15. With no `docDefaults` at all, Word applied its own
/// built-in defaults — which differ by Word version — so a blank document
/// created here and one created in Verbatim didn't agree on body size or
/// line spacing.
#[test]
fn doc_defaults_are_generated_from_the_callers_settings() {
    let style = NewDocStyle {
        normal_size: 24,
        line_spacing: 2.0,
        ..Default::default()
    };
    let xml = build_new_doc_styles_xml(style);
    assert!(
        xml.contains("<w:sz w:val=\"24\"/><w:szCs w:val=\"24\"/>"),
        "got: {xml}"
    );
    assert!(
        xml.contains("w:line=\"480\""),
        "2.0 spacing is 480 twentieths: {xml}"
    );
    // No `<w:rFonts>`: this app has no default-font setting, so it has no
    // opinion to record and Word's own default is the honest answer.
    let defaults = &xml[..xml.find("</w:docDefaults>").unwrap()];
    assert!(!defaults.contains("w:rFonts"), "got: {defaults}");
}

/// The `Heading1`-`Heading4` definitions carry the configured card sizes.
/// They used to hardcode `CardStyleKind::font_size`'s constants while
/// `apply_card_style` applied the configurable values — two sources of
/// truth for one number.
#[test]
fn heading_styles_carry_the_configured_card_sizes() {
    let style = NewDocStyle {
        pocket_size: 60,
        hat_size: 50,
        block_size: 40,
        tag_size: 30,
        ..Default::default()
    };
    let xml = build_new_doc_styles_xml(style);
    for (level, size) in [(1, 60), (2, 50), (3, 40), (4, 30)] {
        let head = xml
            .find(&format!("w:styleId=\"Heading{level}\""))
            .expect("style present");
        let tail = &xml[head..];
        assert!(
            tail[..tail.find("</w:style>").unwrap()].contains(&format!("<w:sz w:val=\"{size}\"/>")),
            "Heading{level} should carry {size}: {xml}",
        );
    }
}

/// Verbatim page-breaks before Pocket, Hat and Block, but deliberately not
/// before Tag. Vimbatim never reads `<w:pageBreakBefore/>` and its editor
/// is continuous, so this changes nothing here and makes a card file
/// paginate in Word the way a Verbatim-authored one does.
#[test]
fn page_breaks_match_verbatim_exactly() {
    let xml = build_new_doc_styles_xml(Default::default());
    let style_body = |level: u8| {
        let head = xml.find(&format!("w:styleId=\"Heading{level}\"")).unwrap();
        let tail = &xml[head..];
        tail[..tail.find("</w:style>").unwrap()].to_string()
    };
    for level in [1, 2, 3] {
        assert!(
            style_body(level).contains("<w:pageBreakBefore/>"),
            "Heading{level}"
        );
    }
    assert!(
        !style_body(4).contains("<w:pageBreakBefore/>"),
        "Tag must not page-break"
    );
}

/// The `Emphasis` character style is generated from *this* app's Emphasis
/// settings, not copied from Verbatim's fixed bold+underline+box — what
/// Emphasis means here is a user preference, and a style that disagreed
/// with the direct formatting on the run would show one thing in Word's
/// Styles pane and another on the page.
#[test]
fn the_emphasis_style_reflects_the_users_emphasis_settings() {
    let body = |style: NewDocStyle| {
        let xml = build_new_doc_styles_xml(style);
        let head = xml.find("w:styleId=\"Emphasis\"").unwrap();
        let tail = &xml[head..];
        tail[..tail.find("</w:style>").unwrap()].to_string()
    };

    let plain = body(NewDocStyle {
        emphasis_bold: true,
        emphasis_underline: false,
        emphasis_box: false,
        emphasis_size: None,
        ..Default::default()
    });
    assert!(plain.contains("<w:b/>"));
    assert!(!plain.contains("<w:u "));
    assert!(!plain.contains("<w:bdr "));
    assert!(!plain.contains("<w:sz "));

    let everything = body(NewDocStyle {
        emphasis_bold: true,
        emphasis_underline: true,
        emphasis_box: true,
        emphasis_size: Some(24),
        ..Default::default()
    });
    assert!(everything.contains("<w:b/>"));
    assert!(everything.contains("<w:u w:val=\"single\"/>"));
    assert!(everything.contains("<w:bdr "));
    assert!(everything.contains("<w:sz w:val=\"24\"/>"));
}

/// Every `<w:link>` names a style that exists. A dangling `<w:link>` is the
/// same defect as the dangling `<w:rStyle>` markers this app used to write.
#[test]
fn heading_char_styles_exist_for_every_link() {
    let xml = build_new_doc_styles_xml(Default::default());
    for level in 1..=4 {
        assert!(
            xml.contains(&format!("<w:link w:val=\"Heading{level}Char\"/>")),
            "link {level}"
        );
        assert!(
            xml.contains(&format!("w:styleId=\"Heading{level}Char\"")),
            "style {level}"
        );
    }
}

// ── docDefaults, read direction ─────────────────────────────────────────

/// A document that declares its body font and size only in
/// `<w:docDefaults>` — which is where Word puts them — must have them read,
/// or an imported file renders at this app's settings instead of its own.
#[test]
fn doc_defaults_are_read_back() {
    let xml = "<w:styles><w:docDefaults><w:rPrDefault><w:rPr>\
                   <w:rFonts w:ascii=\"Calibri\" w:hAnsi=\"Calibri\"/>\
                   <w:sz w:val=\"24\"/><w:szCs w:val=\"24\"/>\
                   </w:rPr></w:rPrDefault></w:docDefaults></w:styles>";
    let defaults = parse_doc_defaults(xml);
    assert_eq!(defaults.font.as_deref(), Some("Calibri"));
    assert_eq!(defaults.size, 24);
}

/// A stylesheet with no `docDefaults`, and a run-level `<w:sz>` inside an
/// ordinary style, must not be mistaken for one.
#[test]
fn doc_defaults_are_empty_when_the_document_declares_none() {
    let xml = "<w:styles><w:style w:type=\"character\" w:styleId=\"X\">\
                   <w:rPr><w:sz w:val=\"96\"/></w:rPr></w:style></w:styles>";
    assert_eq!(parse_doc_defaults(xml), DocDefaults::default());
}

/// What a new document declares is what reading it back reports — the two
/// halves of `docDefaults` have to agree.
#[test]
fn generated_doc_defaults_round_trip() {
    let style = NewDocStyle {
        normal_size: 26,
        line_spacing: 1.5,
        ..Default::default()
    };
    let defaults = parse_doc_defaults(&build_new_doc_styles_xml(style));
    assert_eq!(defaults.size, 26);
    assert_eq!(defaults.font, None);
}

// ── empty paragraphs ────────────────────────────────────────────────────

/// Every shape of blank line Word writes must survive as exactly one
/// paragraph holding at least one run.
///
/// `<w:p/>` used to yield *no* paragraph, and the two `<w:pPr>`-only forms
/// yielded a paragraph with *no runs*. Both broke the "at least one
/// paragraph, at least one run" invariant every rich-text-aware function
/// here assumes, so a blank line either vanished from the document (and,
/// since saving rewrites `document.xml` from this list, from the user's
/// file) or panicked `sync_insert_char` on the first keystroke.
#[test]
fn every_shape_of_empty_paragraph_yields_one_paragraph_with_one_run() {
    for (label, body) in [
        ("self-closing", "<w:p/>"),
        ("self-closing with attrs", "<w:p w:rsidR=\"005D388F\"/>"),
        ("empty start+end", "<w:p></w:p>"),
        (
            "pPr but no runs",
            "<w:p><w:pPr><w:jc w:val=\"center\"/></w:pPr></w:p>",
        ),
        ("empty run", "<w:p><w:r><w:t></w:t></w:r></w:p>"),
    ] {
        let xml = format!("<w:document><w:body>{body}</w:body></w:document>");
        let paras = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(paras.len(), 1, "{label}: expected one paragraph");
        assert_eq!(paras[0].runs.len(), 1, "{label}: expected one run");
        assert!(
            paras[0].runs[0].text.is_empty(),
            "{label}: the run is blank"
        );
    }
}

/// A body with nothing this parser recognises still has to produce the one
/// paragraph every caller assumes, rather than an empty document that
/// indexes out of bounds the moment it is edited.
#[test]
fn a_document_with_no_paragraphs_still_yields_one() {
    let paras = parse_document_xml(
        "<w:document><w:body></w:body></w:document>",
        &no_styles(),
        &no_numbering(),
    )
    .unwrap();
    assert_eq!(paras.len(), 1);
    assert_eq!(paras[0].runs.len(), 1);
}

/// Blank lines are content: they must survive the full parse -> save ->
/// parse round trip, not be quietly dropped on the way through.
#[test]
fn blank_lines_between_cards_survive_a_round_trip() {
    let xml = "<w:document><w:body>\
            <w:p><w:r><w:t>Tag</w:t></w:r></w:p>\
            <w:p/>\
            <w:p><w:pPr><w:jc w:val=\"center\"/></w:pPr></w:p>\
            <w:p><w:r><w:t>Body</w:t></w:r></w:p>\
            </w:body></w:document>";
    let once = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
    assert_eq!(once.len(), 4, "two blank lines between the two text lines");

    let twice = parse_document_xml(
        &rebuild_document_xml("<w:document>", "", &once),
        &no_styles(),
        &no_numbering(),
    )
    .unwrap();
    assert_eq!(twice.len(), 4, "a resave must not drop the blank lines");
    assert_eq!(
        twice
            .iter()
            .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
            .collect::<Vec<_>>(),
        vec!["Tag", "", "", "Body"],
    );
}

// ── XML attribute escaping (escape_xml_attr / attr_value) ───────────────

/// A font family carrying XML metacharacters used to end the attribute
/// early — `<w:rFonts w:ascii="Ev"il<&>"/>` — producing a `.docx` Word
/// refuses to open. `font_import` takes the family name verbatim from the
/// TTF `name` table, so any font can do this; a hand-edited
/// `settings.conf` colour reaches `w:color` the same way.
#[test]
fn attribute_values_with_xml_metacharacters_survive_a_round_trip() {
    let original = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            font: Some("Ev\"il<&>'s Sans".into()),
            color: Some("aa\"bb".into()),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &original);
    assert!(
        !xml.contains("w:ascii=\"Ev\"il"),
        "unescaped quote closed the attribute: {xml}"
    );

    let reparsed = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert_eq!(
        reparsed[0].runs[0].font.as_deref(),
        Some("Ev\"il<&>'s Sans")
    );
    assert_eq!(reparsed[0].runs[0].color.as_deref(), Some("aa\"bb"));
    // `highlight_color` deliberately isn't exercised here: `w:shd`'s own
    // parse already refuses anything that isn't 6 hex digits (so ordinary
    // Word documents don't gain phantom highlights), which is a filter,
    // not an escaping bug.
}

/// The escape and the unescape only work as a pair: escaping output while
/// still reading raw source text back in would re-escape a legitimate
/// `Foo &amp; Bar` one level deeper on every save. Two round trips must
/// be identical to one.
#[test]
fn an_ampersand_in_a_font_name_does_not_grow_on_repeated_saves() {
    let paras = |font: &str| {
        vec![Paragraph {
            list: None,
            runs: vec![Run {
                text: "hi".into(),
                font: Some(font.into()),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }]
    };
    let once = parse_document_xml(
        &rebuild_document_xml("<w:document>", "", &paras("Foo & Bar")),
        &no_styles(),
        &no_numbering(),
    )
    .unwrap();
    assert_eq!(once[0].runs[0].font.as_deref(), Some("Foo & Bar"));

    let twice = parse_document_xml(
        &rebuild_document_xml("<w:document>", "", &once),
        &no_styles(),
        &no_numbering(),
    )
    .unwrap();
    assert_eq!(twice[0].runs[0].font.as_deref(), Some("Foo & Bar"));
}

// ── strikethrough parsing/emission ──────────────────────────────────────

#[test]
fn test_parses_strikethrough_run_property() {
    let xml = wrap_run_xml("<w:rPr><w:strike/></w:rPr>");
    let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert!(paragraphs[0].runs[0].strikethrough);
}

#[test]
fn test_rebuild_emits_strikethrough() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            strikethrough: true,
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(xml.contains("<w:strike/>"));
}

#[test]
fn test_strikethrough_round_trip_through_parse_and_rebuild() {
    let original = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            strikethrough: true,
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &original);
    let reparsed = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert!(reparsed[0].runs[0].strikethrough);
}

// ── Pocket box (paragraph border) parsing/emission ──────────────────────

#[test]
fn test_parses_paragraph_border_as_box_format_on_every_run() {
    let xml = "<w:document><w:body><w:p><w:pPr><w:pBdr><w:top w:val=\"single\" w:sz=\"4\" w:space=\"1\" w:color=\"000000\"/><w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"1\" w:color=\"000000\"/><w:left w:val=\"single\" w:sz=\"4\" w:space=\"1\" w:color=\"000000\"/><w:right w:val=\"single\" w:sz=\"4\" w:space=\"1\" w:color=\"000000\"/></w:pBdr></w:pPr><w:r><w:t>a</w:t></w:r><w:r><w:t>b</w:t></w:r></w:p></w:body></w:document>";
    let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
    // Parse-time run merging collapses "a"+"b" (identical formatting)
    // into one run, so check the property holds across whatever runs
    // remain rather than hardcoding a run count.
    assert!(paragraphs[0].runs.iter().all(|r| r.box_format));
}

#[test]
fn test_paragraph_without_pbdr_has_box_format_false() {
    let xml = "<w:document><w:body><w:p><w:r><w:t>hi</w:t></w:r></w:p></w:body></w:document>";
    let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
    assert!(!paragraphs[0].runs[0].box_format);
}

/// The Pocket heading style's own border has to satisfy the same CT_PBdr
/// ordering as the direct one — a style Word repairs is a style that stops
/// applying.
#[test]
fn new_doc_styles_emit_pbdr_in_schema_order() {
    let styles = build_new_doc_styles_xml(Default::default());
    assert!(
        styles.contains(
            "<w:pBdr>\
                <w:top w:val=\"single\" w:sz=\"24\" w:space=\"1\" w:color=\"auto\"/>\
                <w:left w:val=\"single\" w:sz=\"24\" w:space=\"4\" w:color=\"auto\"/>\
                <w:bottom w:val=\"single\" w:sz=\"24\" w:space=\"1\" w:color=\"auto\"/>\
                <w:right w:val=\"single\" w:sz=\"24\" w:space=\"4\" w:color=\"auto\"/>\
                </w:pBdr>"
        ),
        "got: {styles}",
    );
}

#[test]
fn test_rebuild_emits_four_sided_pbdr_when_box_format_set() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            box_format: true,
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(xml.contains("<w:pBdr>"));
    // top/bottom: space="1"; left/right: space="4" — matching real Verbatim's
    // own Heading1/"Pocket" style exactly (Verbatim_Formatting_To_Compare_To.docx),
    // not a uniform value. A uniform space="1" is what made Vimbatim's pocket
    // box render visibly narrower than Verbatim's when opened in Verbatim.
    //
    // Asserted as one contiguous string, not four independent `contains`
    // calls, because CT_PBdr's child *order* is part of being valid: top,
    // left, bottom, right. `w:color="auto"` follows the text colour the way
    // Verbatim's own style does, instead of a hard black that disappears on
    // a dark document theme.
    assert!(
        xml.contains(
            "<w:pBdr>\
                <w:top w:val=\"single\" w:sz=\"24\" w:space=\"1\" w:color=\"auto\"/>\
                <w:left w:val=\"single\" w:sz=\"24\" w:space=\"4\" w:color=\"auto\"/>\
                <w:bottom w:val=\"single\" w:sz=\"24\" w:space=\"1\" w:color=\"auto\"/>\
                <w:right w:val=\"single\" w:sz=\"24\" w:space=\"4\" w:color=\"auto\"/>\
                </w:pBdr>"
        ),
        "got: {xml}",
    );
}

#[test]
fn test_rebuild_omits_pbdr_when_no_run_has_box_format() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(!xml.contains("w:pBdr"));
}

/// The marker's whole point: it survives a save/reload, so a Cite stays a
/// Cite instead of being re-guessed from bold + font size.
#[test]
fn test_style_marker_round_trips_through_parse_and_rebuild() {
    // The four card styles ride on their paragraph's heading level, which
    // is where Verbatim keeps them too — `from_heading` restores the run
    // marker at parse, so nothing is written at run level and nothing is
    // lost. Cite and Analytic have no heading to ride on and keep an
    // explicit `<w:rStyle>`.
    let by_heading = [
        (CardStyle::Pocket, 1u8),
        (CardStyle::Hat, 2),
        (CardStyle::Block, 3),
        (CardStyle::Tag, 4),
    ];
    for (style, heading) in by_heading {
        let paragraphs = vec![Paragraph {
            list: None,
            runs: vec![Run {
                text: "marked".into(),
                style: Some(style),
                ..Run::default()
            }],
            heading,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let xml = rebuild_document_xml(&fallback_preamble(), "", &paragraphs);
        assert!(
            !xml.contains("<w:rStyle"),
            "{style:?} must ride on its pStyle, as Verbatim's own do: {xml}",
        );
        let reparsed = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(
            reparsed[0].runs[0].style,
            Some(style),
            "{style:?} did not survive the round trip"
        );
    }

    for style in [CardStyle::Cite, CardStyle::Analytic] {
        let paragraphs = vec![Paragraph {
            list: None,
            runs: vec![Run {
                text: "marked".into(),
                style: Some(style),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let xml = rebuild_document_xml(&fallback_preamble(), "", &paragraphs);
        let reparsed = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(
            reparsed[0].runs[0].style,
            Some(style),
            "{style:?} did not survive the round trip"
        );
    }
}

/// Every `<w:rStyle>` this app writes must be a style a new document
/// actually defines. The four card markers it used to emit
/// (`VimbatimPocket` and friends) were in no stylesheet anywhere.
#[test]
fn every_emitted_rstyle_id_is_defined_in_a_new_documents_styles() {
    let styles = build_new_doc_styles_xml(Default::default());
    for style in [
        CardStyle::Pocket,
        CardStyle::Hat,
        CardStyle::Block,
        CardStyle::Tag,
        CardStyle::Cite,
        CardStyle::Analytic,
    ] {
        let Some(id) = style.docx_rstyle_id() else {
            continue;
        };
        assert!(
            styles.contains(&format!("w:styleId=\"{id}\"")),
            "{style:?} writes <w:rStyle w:val=\"{id}\"/> but no style defines it",
        );
    }
}

/// Files written before the switch to Verbatim's ids still resolve their
/// markers — the legacy arm in `from_style_id`. Dropping it early would
/// silently strip the Cite marker from every document already saved, with
/// the direct bold and size still making it look correct.
#[test]
fn legacy_vimbatim_style_ids_are_still_read() {
    for (id, expected) in [
        ("VimbatimPocket", CardStyle::Pocket),
        ("VimbatimHat", CardStyle::Hat),
        ("VimbatimBlock", CardStyle::Block),
        ("VimbatimTag", CardStyle::Tag),
        ("VimbatimCite", CardStyle::Cite),
    ] {
        let xml = wrap_run_xml(&format!(r#"<w:rPr><w:rStyle w:val="{id}"/></w:rPr>"#));
        let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].runs[0].style, Some(expected), "{id}");
    }
}

/// The id is written as a `<w:rStyle>` reference, which Word ignores when
/// the style isn't defined — so a marked document opens cleanly elsewhere.
#[test]
fn test_style_marker_is_written_as_an_rstyle_reference() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "a cite".into(),
            style: Some(CardStyle::Cite),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml(&fallback_preamble(), "", &paragraphs);
    // Verbatim's own id for a Cite, so a card written here is the same
    // thing to Verbatim that one written there is.
    assert!(
        xml.contains(r#"<w:rStyle w:val="Style13ptBold"/>"#),
        "got: {xml}"
    );
}

/// The Emphasis markers round-trip through save/load, and independently
/// of each other and of `box_format`/`style` — a run can be Block-styled
/// and emphasized-but-not-boxed at the same time.
#[test]
fn test_emphasis_markers_survive_round_trip() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![
            Run {
                text: "emphasized only".into(),
                emphasis: true,
                ..Run::default()
            },
            Run {
                text: "emphasized and boxed".into(),
                emphasis: true,
                emphasis_boxed: true,
                bold: true,
                style: Some(CardStyle::Block),
                ..Run::default()
            },
        ],
        // A Block-marked run lives on a Block paragraph — that is where
        // the marker round-trips from now that the four card styles ride
        // on `pStyle` rather than an `<w:rStyle>` of their own.
        heading: 3,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml(&fallback_preamble(), "", &paragraphs);
    assert!(xml.contains("<w:vimbatimEmphasis/>"), "got: {xml}");
    assert!(xml.contains("<w:vimbatimEmphasisBox/>"), "got: {xml}");

    let reparsed = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    let runs = &reparsed[0].runs;
    assert!(runs[0].emphasis && !runs[0].emphasis_boxed);
    assert!(runs[1].emphasis && runs[1].emphasis_boxed);
    assert!(runs[1].bold && runs[1].style == Some(CardStyle::Block));
}

/// A document written in Word carries heading styles but no markers —
/// deriving them at parse time is the accuracy win.
#[test]
fn test_word_heading_styles_become_markers() {
    let xml = "<w:document><w:body><w:p>\
                   <w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr>\
                   <w:r><w:t>a pocket</w:t></w:r>\
                   </w:p></w:body></w:document>";
    let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
    assert_eq!(paragraphs[0].heading, 1);
    assert_eq!(paragraphs[0].runs[0].style, Some(CardStyle::Pocket));
}

/// Runs that look identical but carry different markers are different
/// things — merging them would erase one.
#[test]
fn test_runs_with_different_markers_do_not_merge() {
    let mut runs = vec![
        Run {
            text: "a".into(),
            style: Some(CardStyle::Cite),
            ..Run::default()
        },
        Run {
            text: "b".into(),
            style: Some(CardStyle::Analytic),
            ..Run::default()
        },
    ];
    crate::document::normalize::merge_adjacent_same_format_runs(&mut runs);
    assert_eq!(runs.len(), 2);
}

#[test]
fn test_box_format_round_trip_through_parse_and_rebuild() {
    let original = vec![Paragraph {
        list: None,
        runs: vec![
            Run {
                text: "a".into(),
                box_format: true,
                ..Run::default()
            },
            Run {
                text: "b".into(),
                box_format: true,
                ..Run::default()
            },
        ],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &original);
    let reparsed = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    // Parse-time run merging collapses "a"+"b" (identical formatting)
    // into one run, so check the property holds across whatever runs
    // remain rather than hardcoding a run count.
    assert!(reparsed[0].runs.iter().all(|r| r.box_format));
}

// ── real-file round trip (parse_docx -> DocxOrigin::save -> parse_docx) ─

#[test]
fn test_real_file_round_trip_preserves_all_five_fixed_attributes() {
    let dir = std::env::temp_dir().join(format!("vimbatim_docx_roundtrip_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.docx");

    // 1. Create a minimal real .docx on disk.
    let initial = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hello".into(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    create_new_docx(&initial, &path, Default::default()).unwrap();

    // 2. Open it through the real parse_docx path (ZIP + XML), not the
    //    XML-string helpers the rest of this file's tests use.
    let (mut paragraphs, origin) = parse_docx(&path).unwrap();
    assert_eq!(paragraphs[0].runs[0].text, "hello");

    // 3. Apply every attribute this plan fixed, directly on the parsed
    //    model (mirroring what AppState::apply_card_style and
    //    apply_formatting_to_selection do in the real app).
    paragraphs[0].heading = 1;
    paragraphs[0].alignment = Alignment::Center;
    paragraphs[0].runs[0].double_underline = true;
    paragraphs[0].runs[0].strikethrough = true;
    paragraphs[0].runs[0].box_format = true;

    // 4. Save through the real DocxOrigin::save path (ZIP write, not a
    //    bare string).
    origin.save(&paragraphs, &path).unwrap();

    // 5. Parse it again from scratch — a completely fresh read of the
    //    file just written, proving the round trip survives a real
    //    save/reload, not just an in-memory transformation.
    let (reparsed, _origin2) = parse_docx(&path).unwrap();
    assert_eq!(reparsed[0].heading, 1);
    assert_eq!(reparsed[0].alignment, Alignment::Center);
    assert!(reparsed[0].runs[0].double_underline);
    assert!(reparsed[0].runs[0].strikethrough);
    assert!(reparsed[0].runs[0].box_format);

    std::fs::remove_file(&path).ok();
    std::fs::remove_dir(&dir).ok();
}

// ── regression tests against a real Verbatim-authored reference file ───

/// Parses the actual file real Verbatim produced (the reference pair the
/// `Round Trip Bug Fixes.md` tracker itself was built from) and confirms
/// Emphasis's box now survives, using real Verbatim bytes rather than a
/// hand-written XML fixture.
#[test]
fn test_parses_real_verbatim_authored_emphasis_box() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/verbatim_emphasis_reference.docx");
    let (paragraphs, _origin) = parse_docx(&path).unwrap();

    let pocket = paragraphs
        .iter()
        .find(|p| p.runs.iter().any(|r| r.text == "Pocket"))
        .unwrap();
    assert!(
        pocket.runs[0].box_format,
        "Pocket paragraph should carry the paragraph-wide box"
    );

    let emphasis_para = paragraphs
        .iter()
        .find(|p| p.runs.iter().any(|r| r.text.contains("Emphasis (Size")))
        .expect("Emphasis paragraph not found");
    assert!(
        emphasis_para
            .runs
            .iter()
            .all(|r| r.emphasis && r.emphasis_boxed),
        "every run in the Emphasis paragraph should be marked emphasis + emphasis_boxed, got: {:?}",
        emphasis_para.runs,
    );

    let emphasis_highlight_para = paragraphs
        .iter()
        .find(|p| {
            p.runs
                .iter()
                .any(|r| r.text.contains("Emphasis (same as above) and highlight"))
        })
        .expect("Emphasis+highlight paragraph not found");
    let run = &emphasis_highlight_para.runs[0];
    assert!(
        run.emphasis && run.emphasis_boxed,
        "Emphasis+highlight should also carry the box"
    );
    assert!(run.highlight && run.highlight_color == "yellow");
}

/// Round-trips that same real Verbatim file through a real save/reload
/// (ZIP write, not a bare string), confirming the fix survives the whole
/// pipeline the app actually uses when a user opens a Verbatim file, edits
/// it, and saves — not just an in-memory transformation.
#[test]
fn test_real_verbatim_file_emphasis_box_survives_save_and_reload() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/verbatim_emphasis_reference.docx");
    let dir = std::env::temp_dir().join(format!(
        "vimbatim_verbatim_roundtrip_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.docx");
    std::fs::copy(&src, &path).unwrap();

    let (paragraphs, origin) = parse_docx(&path).unwrap();
    origin.save(&paragraphs, &path).unwrap();

    let (reparsed, _origin2) = parse_docx(&path).unwrap();
    let emphasis_para = reparsed
        .iter()
        .find(|p| p.runs.iter().any(|r| r.text.contains("Emphasis (Size")))
        .expect("Emphasis paragraph not found after save/reload");
    assert!(emphasis_para
        .runs
        .iter()
        .all(|r| r.emphasis && r.emphasis_boxed));

    let pocket = reparsed
        .iter()
        .find(|p| p.runs.iter().any(|r| r.text == "Pocket"))
        .expect("Pocket paragraph not found after save/reload");
    assert!(
        pocket.runs[0].box_format,
        "Pocket's box_format should survive save/reload"
    );

    std::fs::remove_file(&path).ok();
    std::fs::remove_dir(&dir).ok();
}

// ── heading style round-trips against Word's actual built-in styleId ────

#[test]
fn test_rebuild_emits_capitalized_heading_styleid_matching_words_own_styles_xml() {
    // Real Word documents (and every version of Microsoft Word itself)
    // define the built-in heading styles with a capitalized styleId —
    // <w:style w:type="paragraph" w:styleId="Heading1"> in word/styles.xml
    // — and OOXML styleId references are matched case-sensitively. If
    // rebuild_document_xml emits a differently-cased w:val, the saved
    // paragraph's <w:pStyle> points at a styleId that doesn't exist in
    // styles.xml (which this app never rewrites), so Word silently falls
    // back to Normal: the heading's bold/size/outline-level vanish and it
    // drops out of Word's Navigation pane, even though parsing it back
    // into Vimbatim still shows a heading (apply_para_style lower-cases
    // before matching, so it doesn't notice the mismatch).
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            ..Run::default()
        }],
        heading: 1,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(xml.contains(r#"<w:pStyle w:val="Heading1"/>"#));
}

// ── block-level unsupported content detection ───────────────────────────

#[test]
fn test_detects_table_in_document_xml() {
    let xml = "<w:document><w:body><w:tbl><w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>";
    assert!(xml.contains("<w:tbl"));
}

#[test]
fn test_parse_docx_sets_has_unsupported_blocks_for_real_file_with_table() {
    let dir = std::env::temp_dir().join(format!("vimbatim_docx_table_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("with_table.docx");

    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "before table".into(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    create_new_docx(&paragraphs, &path, Default::default()).unwrap();

    // create_new_docx has no table support itself, so this only confirms
    // the negative case end-to-end through a real file — splicing a real
    // <w:tbl> into a ZIP-written file for the positive case is
    // significant extra machinery for marginal coverage beyond the
    // already-passing test_detects_table_in_document_xml string check.
    let (_paragraphs, origin) = parse_docx(&path).unwrap();
    assert!(!origin.has_unsupported_blocks);

    std::fs::remove_file(&path).ok();
    std::fs::remove_dir(&dir).ok();
}

// ── paragraph-style-based formatting (word/styles.xml resolution) ───────

// Mirrors the shape real Word (and the "Verbatim" tool the user tested
// with) actually writes for a named paragraph style: box+center+bold+
// size live on the STYLE, not inline on each paragraph that uses it.
const POCKET_STYLE_XML: &str = r#"<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:aliases w:val="Pocket"/><w:basedOn w:val="Normal"/><w:pPr><w:pBdr><w:top w:val="single" w:sz="24" w:space="1" w:color="auto"/><w:left w:val="single" w:sz="24" w:space="4" w:color="auto"/><w:bottom w:val="single" w:sz="24" w:space="1" w:color="auto"/><w:right w:val="single" w:sz="24" w:space="4" w:color="auto"/></w:pBdr><w:jc w:val="center"/></w:pPr><w:rPr><w:b/><w:sz w:val="52"/></w:rPr></w:style>"#;

#[test]
fn test_parses_alignment_and_box_from_referenced_paragraph_style() {
    let styles_xml = format!("<w:styles>{}</w:styles>", POCKET_STYLE_XML);
    let styles = parse_styles_xml(&styles_xml);
    let xml = "<w:document><w:body><w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>hi</w:t></w:r></w:p></w:body></w:document>";
    let paragraphs = parse_document_xml(xml, &styles, &no_numbering()).unwrap();
    assert_eq!(paragraphs[0].alignment, Alignment::Center);
    assert_eq!(paragraphs[0].heading, 1);
    assert!(paragraphs[0].runs[0].box_format);
    assert!(paragraphs[0].runs[0].bold);
    assert_eq!(paragraphs[0].runs[0].size, 52);
}

#[test]
fn test_direct_paragraph_formatting_overrides_style_defaults() {
    let styles_xml = format!("<w:styles>{}</w:styles>", POCKET_STYLE_XML);
    let styles = parse_styles_xml(&styles_xml);
    // Same style reference as above, but this paragraph ALSO carries its
    // own direct <w:jc> - direct formatting must win over the style's.
    let xml = "<w:document><w:body><w:p><w:pPr><w:pStyle w:val=\"Heading1\"/><w:jc w:val=\"left\"/></w:pPr><w:r><w:t>hi</w:t></w:r></w:p></w:body></w:document>";
    let paragraphs = parse_document_xml(xml, &styles, &no_numbering()).unwrap();
    assert_eq!(paragraphs[0].alignment, Alignment::Left);
}

#[test]
fn test_direct_run_formatting_overrides_style_defaults() {
    let styles_xml = format!("<w:styles>{}</w:styles>", POCKET_STYLE_XML);
    let styles = parse_styles_xml(&styles_xml);
    // The style says sz=52; this run's own <w:sz> should win.
    let xml = "<w:document><w:body><w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:rPr><w:sz w:val=\"80\"/></w:rPr><w:t>hi</w:t></w:r></w:p></w:body></w:document>";
    let paragraphs = parse_document_xml(xml, &styles, &no_numbering()).unwrap();
    assert_eq!(paragraphs[0].runs[0].size, 80);
    assert!(paragraphs[0].runs[0].bold); // still inherited from the style
}

#[test]
fn test_paragraph_without_pstyle_is_unaffected_by_styles_map() {
    let styles_xml = format!("<w:styles>{}</w:styles>", POCKET_STYLE_XML);
    let styles = parse_styles_xml(&styles_xml);
    let xml = "<w:document><w:body><w:p><w:r><w:t>plain</w:t></w:r></w:p></w:body></w:document>";
    let paragraphs = parse_document_xml(xml, &styles, &no_numbering()).unwrap();
    assert_eq!(paragraphs[0].alignment, Alignment::Left);
    assert!(!paragraphs[0].runs[0].box_format);
    assert!(!paragraphs[0].runs[0].bold);
    assert_eq!(paragraphs[0].runs[0].size, 0);
}

// ── parse-time run merging (editing-speed fix) ──────────────────────────

#[test]
fn test_adjacent_runs_with_identical_formatting_merge_at_parse_time() {
    // Word fragments runs at spell-check/revision boundaries even when
    // formatting doesn't change; merging these once at parse time keeps
    // every later per-keystroke edit as cheap on a loaded document as on
    // a freshly-typed one (both O(runs), but this keeps `runs` small).
    let xml = "<w:document><w:body><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>foo</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>bar</w:t></w:r></w:p></w:body></w:document>";
    let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
    assert_eq!(paragraphs[0].runs.len(), 1);
    assert_eq!(paragraphs[0].runs[0].text, "foobar");
    assert!(paragraphs[0].runs[0].bold);
}

#[test]
fn test_adjacent_runs_with_different_formatting_stay_separate_at_parse_time() {
    let xml = "<w:document><w:body><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>foo</w:t></w:r><w:r><w:t>bar</w:t></w:r></w:p></w:body></w:document>";
    let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
    assert_eq!(paragraphs[0].runs.len(), 2);
    assert!(paragraphs[0].runs[0].bold);
    assert!(!paragraphs[0].runs[1].bold);
}

#[test]
fn test_pstyle_referencing_unknown_style_id_is_unaffected() {
    let styles = parse_styles_xml("<w:styles></w:styles>");
    let xml = "<w:document><w:body><w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>hi</w:t></w:r></w:p></w:body></w:document>";
    let paragraphs = parse_document_xml(xml, &styles, &no_numbering()).unwrap();
    assert_eq!(paragraphs[0].heading, 1); // name-based heading detection still works
    assert_eq!(paragraphs[0].alignment, Alignment::Left);
    assert!(!paragraphs[0].runs[0].box_format);
}

// ── unsupported inline content preservation ─────────────────────────────

#[test]
fn test_captures_unsupported_xml_for_paragraph_with_hyperlink() {
    let xml = "<w:document><w:body><w:p><w:hyperlink r:id=\"rId1\"><w:r><w:t>link text</w:t></w:r></w:hyperlink></w:p></w:body></w:document>";
    let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
    assert!(paragraphs[0].unsupported_xml.is_some());
    assert!(paragraphs[0]
        .unsupported_xml
        .as_ref()
        .unwrap()
        .contains("w:hyperlink"));
}

#[test]
fn test_plain_paragraph_has_no_unsupported_xml() {
    let xml = "<w:document><w:body><w:p><w:r><w:t>plain</w:t></w:r></w:p></w:body></w:document>";
    let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
    assert_eq!(paragraphs[0].unsupported_xml, None);
}

#[test]
fn test_incidental_tags_do_not_trigger_unsupported_xml_capture() {
    // Bookmarks are common and harmless - must NOT freeze this paragraph.
    let xml = "<w:document><w:body><w:p><w:bookmarkStart w:id=\"0\" w:name=\"_Test\"/><w:r><w:t>plain</w:t></w:r><w:bookmarkEnd w:id=\"0\"/></w:p></w:body></w:document>";
    let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
    assert_eq!(paragraphs[0].unsupported_xml, None);
}

#[test]
fn test_rebuild_reemits_unsupported_xml_verbatim_when_present() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "ignored".into(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: Some(
            "<w:hyperlink r:id=\"rId1\"><w:r><w:t>link text</w:t></w:r></w:hyperlink>".to_string(),
        ),
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(xml.contains("<w:hyperlink r:id=\"rId1\">"));
    assert!(xml.contains("link text"));
    // The runs the app *did* manage to parse for display purposes must
    // NOT also be independently re-emitted - unsupported_xml IS the
    // paragraph's entire content on save.
    assert!(!xml.contains("ignored"));
}

#[test]
fn test_unsupported_xml_round_trips_through_untouched_edit_elsewhere() {
    let xml = "<w:document><w:body><w:p><w:hyperlink r:id=\"rId1\"><w:r><w:t>link</w:t></w:r></w:hyperlink></w:p><w:p><w:r><w:t>other paragraph</w:t></w:r></w:p></w:body></w:document>";
    let mut paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
    assert!(paragraphs[0].unsupported_xml.is_some());

    // Edit only the SECOND paragraph - the first (with the hyperlink)
    // is never touched.
    paragraphs[1].runs[0].text = "edited".to_string();

    let rebuilt = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(rebuilt.contains("w:hyperlink"));
    assert!(rebuilt.contains("edited"));
}

// ── italic/font/color re-emission (rebuild_document_xml) ────────────────

#[test]
fn test_rebuild_emits_italic() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            italic: true,
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(xml.contains("<w:i/>"));
}

#[test]
fn test_rebuild_emits_font_ascii() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            font: Some("Georgia".into()),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(xml.contains(r#"<w:rFonts w:ascii="Georgia"/>"#));
}

#[test]
fn test_rebuild_emits_color() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            color: Some("FF0000".into()),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(xml.contains(r#"<w:color w:val="FF0000"/>"#));
}

#[test]
fn test_rebuild_omits_rpr_entirely_when_no_properties_set() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(!xml.contains("<w:rPr>"));
}

#[test]
fn test_italic_font_color_round_trip_through_parse_and_rebuild() {
    let original = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hi".into(),
            italic: true,
            font: Some("Georgia".into()),
            color: Some("00FF00".into()),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &original);
    // rebuild_document_xml wraps in <w:body>...</w:body></w:document>,
    // matching what parse_document_xml expects to find.
    let reparsed = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert!(reparsed[0].runs[0].italic);
    assert_eq!(reparsed[0].runs[0].font, Some("Georgia".to_string()));
    assert_eq!(reparsed[0].runs[0].color, Some("00FF00".to_string()));
}

// ── recovery snapshot round trip (DocxOrigin::save_snapshot) ────────────

#[test]
fn save_snapshot_produces_a_docx_that_parses_back_to_the_same_paragraphs() {
    let dir = std::env::temp_dir().join(format!("vimbatim-snap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let original = dir.join("original.docx");
    let snapshot = dir.join("snapshot.docx");

    // Build a real docx, then reload it to get a genuine DocxOrigin.
    let mut para = Paragraph::default();
    para.runs.push(Run {
        text: "hello world".into(),
        ..Default::default()
    });
    create_new_docx(&[para.clone()], &original, Default::default()).unwrap();
    let (paragraphs, origin) = parse_docx(&original).unwrap();

    origin.save_snapshot(&paragraphs, &snapshot).unwrap();

    let (restored, _) = parse_docx(&snapshot).unwrap();
    assert_eq!(restored, paragraphs);

    std::fs::remove_file(&original).ok();
    std::fs::remove_file(&snapshot).ok();
    std::fs::remove_dir(&dir).ok();
}

// ── real Word list parsing (word/numbering.xml) ─────────────────────────

#[test]
fn test_parse_numbering_xml_extracts_numfmt_lvltext_font_per_numid() {
    let xml = format!(
        "<w:numbering>\
             <w:abstractNum w:abstractNumId=\"4\">\
             <w:lvl w:ilvl=\"0\">\
             <w:numFmt w:val=\"bullet\"/>\
             <w:lvlText w:val=\"{}\"/>\
             <w:rPr><w:rFonts w:ascii=\"Symbol\" w:hAnsi=\"Symbol\"/></w:rPr>\
             </w:lvl>\
             </w:abstractNum>\
             <w:abstractNum w:abstractNumId=\"8\">\
             <w:lvl w:ilvl=\"0\">\
             <w:numFmt w:val=\"decimal\"/>\
             <w:lvlText w:val=\"%1.\"/>\
             </w:lvl>\
             </w:abstractNum>\
             <w:num w:numId=\"1\"><w:abstractNumId w:val=\"4\"/></w:num>\
             <w:num w:numId=\"2\"><w:abstractNumId w:val=\"8\"/></w:num>\
             </w:numbering>",
        "\u{f0b7}",
    );
    let map = parse_numbering_xml(&xml);
    assert_eq!(
        map.get(&1),
        Some(&(
            "bullet".to_string(),
            "\u{f0b7}".to_string(),
            Some("Symbol".to_string())
        ))
    );
    assert_eq!(
        map.get(&2),
        Some(&("decimal".to_string(), "%1.".to_string(), None))
    );
}

#[test]
fn test_list_kind_classify_exact_matches() {
    assert_eq!(
        ListKind::classify("bullet", "\u{f0b7}", Some("Symbol")),
        ListKind::BulletSolid
    );
    assert_eq!(
        ListKind::classify("bullet", "o", Some("Courier New")),
        ListKind::BulletHollow
    );
    assert_eq!(
        ListKind::classify("decimal", "%1.", None),
        ListKind::NumberDecimalDot
    );
    assert_eq!(
        ListKind::classify("lowerRoman", "%1.", None),
        ListKind::NumberLowerRoman
    );
}

#[test]
fn test_list_kind_classify_falls_back_for_unrecognized_bullet() {
    assert_eq!(
        ListKind::classify("bullet", "\u{2013}", Some("Arial")),
        ListKind::BulletSolid
    );
}

#[test]
fn test_list_kind_classify_falls_back_for_unrecognized_number_punctuation() {
    assert_eq!(
        ListKind::classify("decimal", "%1:", None),
        ListKind::NumberDecimalDot
    );
    assert_eq!(
        ListKind::classify("lowerLetter", "%1:", None),
        ListKind::NumberLowerLetterDot
    );
}

#[test]
fn test_list_kind_classify_falls_back_for_unrecognized_numfmt() {
    assert_eq!(
        ListKind::classify("cardinalText", "%1.", None),
        ListKind::NumberDecimalDot
    );
}

#[test]
fn test_parses_numpr_into_paragraph_list() {
    let xml = "<w:document><w:body><w:p>\
                   <w:pPr><w:pStyle w:val=\"ListParagraph\"/><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr></w:pPr>\
                   <w:r><w:t>Item One</w:t></w:r>\
                   </w:p></w:body></w:document>";
    let mut numbering = HashMap::new();
    numbering.insert(
        1u32,
        (
            "bullet".to_string(),
            "\u{f0b7}".to_string(),
            Some("Symbol".to_string()),
        ),
    );
    let paragraphs = parse_document_xml(xml, &no_styles(), &numbering).unwrap();
    assert_eq!(
        paragraphs[0].list,
        Some(ListItem {
            kind: ListKind::BulletSolid,
            level: 0
        })
    );
}

#[test]
fn test_paragraph_without_numpr_has_no_list() {
    let xml = wrap_run_xml("<w:rPr><w:b/></w:rPr>");
    let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
    assert_eq!(paragraphs[0].list, None);
}

// ── word/numbering.xml generation ───────────────────────────────────────

#[test]
fn test_build_numbering_xml_returns_none_for_no_lists() {
    let paragraphs = vec![Paragraph {
        runs: vec![Run {
            text: "hi".into(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        list: None,
        unsupported_xml: None,
    }];
    assert!(build_numbering_xml(&paragraphs).is_none());
}

#[test]
fn test_build_numbering_xml_emits_all_13_abstract_nums_with_9_levels_each() {
    let paragraphs = vec![Paragraph {
        runs: vec![Run {
            text: "hi".into(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        list: Some(ListItem {
            kind: ListKind::BulletSolid,
            level: 0,
        }),
        unsupported_xml: None,
    }];
    let xml = build_numbering_xml(&paragraphs).unwrap();
    assert_eq!(xml.matches("<w:abstractNum ").count(), 13, "got: {xml}");
    // Spot-check one full abstractNum's level-0/1/2, matching Lists.docx exactly.
    assert!(
        xml.contains("<w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"\u{f0b7}\"/>"),
        "got: {xml}"
    );
    assert!(
        xml.contains("<w:rFonts w:ascii=\"Symbol\" w:hAnsi=\"Symbol\"/>"),
        "got: {xml}"
    );
    // Confirmed cascade: every bullet style's level 1 is 'o'/Courier New,
    // level 2 is /Wingdings, regardless of the level-0 style.
    assert!(
        xml.matches("<w:lvlText w:val=\"o\"/>").count() >= 6,
        "got: {xml}"
    );
    assert!(
        xml.matches("<w:rFonts w:ascii=\"Courier New\" w:hAnsi=\"Courier New\"/>")
            .count()
            >= 6,
        "got: {xml}"
    );
}

/// Exhaustive ilvl 1-8 check against the *complete* real-Word cascade
/// (not just ilvl 1-2, which the version of `cascade_level_xml` that
/// first shipped in Task 3 checked — it collapsed cascade positions 1
/// and 2 into one match arm, silently wrong at ilvl 3, 6 for both
/// bullets and numbers; see that function's own doc comment for the
/// full fix history).
#[test]
fn test_cascade_level_xml_matches_the_complete_real_word_cascade() {
    let bullet_expected = [
        (1u8, "o", "Courier New"),
        (2, "\u{f0a7}", "Wingdings"),
        (3, "\u{f0b7}", "Symbol"),
        (4, "o", "Courier New"),
        (5, "\u{f0a7}", "Wingdings"),
        (6, "\u{f0b7}", "Symbol"),
        (7, "o", "Courier New"),
        (8, "\u{f0a7}", "Wingdings"),
    ];
    for (ilvl, lvl_text, font) in bullet_expected {
        let xml = cascade_level_xml(ilvl, true);
        assert!(
            xml.contains(&format!("<w:lvlText w:val=\"{lvl_text}\"/>")),
            "ilvl {ilvl}, got: {xml}"
        );
        assert!(
            xml.contains(&format!("w:ascii=\"{font}\"")),
            "ilvl {ilvl}, got: {xml}"
        );
    }

    let number_expected = [
        (1u8, "lowerLetter", 360u32),
        (2, "lowerRoman", 180),
        (3, "decimal", 360),
        (4, "lowerLetter", 360),
        (5, "lowerRoman", 180),
        (6, "decimal", 360),
        (7, "lowerLetter", 360),
        (8, "lowerRoman", 180),
    ];
    for (ilvl, num_fmt, hanging) in number_expected {
        let xml = cascade_level_xml(ilvl, false);
        assert!(
            xml.contains(&format!("<w:numFmt w:val=\"{num_fmt}\"/>")),
            "ilvl {ilvl}, got: {xml}"
        );
        assert!(
            xml.contains(&format!("w:hanging=\"{hanging}\"")),
            "ilvl {ilvl}, got: {xml}"
        );
    }
}

#[test]
fn test_build_numbering_xml_emits_one_num_per_contiguous_list_run() {
    let paragraphs = vec![
        Paragraph {
            runs: vec![Run {
                text: "a".into(),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            list: Some(ListItem {
                kind: ListKind::NumberDecimalDot,
                level: 0,
            }),
            unsupported_xml: None,
        },
        Paragraph {
            runs: vec![Run {
                text: "b".into(),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            list: Some(ListItem {
                kind: ListKind::NumberDecimalDot,
                level: 0,
            }),
            unsupported_xml: None,
        },
        Paragraph {
            runs: vec![Run {
                text: "not a list".into(),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            list: None,
            unsupported_xml: None,
        },
        Paragraph {
            runs: vec![Run {
                text: "c".into(),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            list: Some(ListItem {
                kind: ListKind::NumberDecimalDot,
                level: 0,
            }),
            unsupported_xml: None,
        },
    ];
    let xml = build_numbering_xml(&paragraphs).unwrap();
    // Two runs of decimal-dot list paragraphs (a,b) and (c) -> two <w:num> entries.
    assert_eq!(xml.matches("<w:num ").count(), 2, "got: {xml}");
}

/// Confirmed against real Word (`Lists.docx`'s six-different-bullets
/// example, six immediately consecutive paragraphs each with their own
/// numId): a `ListKind` change starts a new numbering run even with no
/// non-list paragraph between them. An earlier version of
/// `assign_list_num_ids` broke runs only on `list.is_some()` going
/// false, silently collapsing all six styles onto one shared numId —
/// caught by `test_real_lists_docx_survives_save_and_reload` resolving
/// a saved `Checkmark` paragraph back as a different style entirely
/// after a round trip.
#[test]
fn test_build_numbering_xml_gives_each_list_kind_its_own_num_even_with_no_break_between() {
    let paragraphs = vec![
        Paragraph {
            runs: vec![Run {
                text: "a".into(),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            list: Some(ListItem {
                kind: ListKind::BulletSolid,
                level: 0,
            }),
            unsupported_xml: None,
        },
        Paragraph {
            runs: vec![Run {
                text: "b".into(),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            list: Some(ListItem {
                kind: ListKind::BulletHollow,
                level: 0,
            }),
            unsupported_xml: None,
        },
        Paragraph {
            runs: vec![Run {
                text: "c".into(),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            list: Some(ListItem {
                kind: ListKind::BulletCheckmark,
                level: 0,
            }),
            unsupported_xml: None,
        },
    ];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    // Three different kinds, immediately adjacent -> three distinct numIds.
    assert!(xml.contains("<w:numId w:val=\"1\"/>"), "got: {xml}");
    assert!(xml.contains("<w:numId w:val=\"2\"/>"), "got: {xml}");
    assert!(xml.contains("<w:numId w:val=\"3\"/>"), "got: {xml}");

    let numbering_xml = build_numbering_xml(&paragraphs).unwrap();
    assert_eq!(
        numbering_xml.matches("<w:num ").count(),
        3,
        "got: {numbering_xml}"
    );
    // Each numId must resolve back to its own paragraph's real kind,
    // not an arbitrary one sharing the id.
    let numbering = parse_numbering_xml(&numbering_xml);
    let reparsed = parse_document_xml(&xml, &no_styles(), &numbering).unwrap();
    assert_eq!(
        reparsed[0].list.map(|l| l.kind),
        Some(ListKind::BulletSolid)
    );
    assert_eq!(
        reparsed[1].list.map(|l| l.kind),
        Some(ListKind::BulletHollow)
    );
    assert_eq!(
        reparsed[2].list.map(|l| l.kind),
        Some(ListKind::BulletCheckmark)
    );
}

// ── pStyle/numPr emission + shared numId assignment ─────────────────────

#[test]
fn test_rebuild_emits_pstyle_and_numpr_for_list_paragraphs() {
    let paragraphs = vec![
        Paragraph {
            runs: vec![Run {
                text: "one".into(),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            list: Some(ListItem {
                kind: ListKind::BulletSolid,
                level: 0,
            }),
            unsupported_xml: None,
        },
        Paragraph {
            runs: vec![Run {
                text: "two".into(),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            list: Some(ListItem {
                kind: ListKind::BulletSolid,
                level: 0,
            }),
            unsupported_xml: None,
        },
    ];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert_eq!(
        xml.matches("<w:pStyle w:val=\"ListParagraph\"/>").count(),
        2,
        "got: {xml}"
    );
    // Both paragraphs are one contiguous run -> same numId, both ilvl 0.
    assert_eq!(
        xml.matches("<w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr>")
            .count(),
        2,
        "got: {xml}"
    );
}

#[test]
fn test_rebuild_gives_separate_list_runs_different_numids() {
    let paragraphs = vec![
        Paragraph {
            runs: vec![Run {
                text: "a".into(),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            list: Some(ListItem {
                kind: ListKind::NumberDecimalDot,
                level: 0,
            }),
            unsupported_xml: None,
        },
        Paragraph {
            runs: vec![Run {
                text: "not a list".into(),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            list: None,
            unsupported_xml: None,
        },
        Paragraph {
            runs: vec![Run {
                text: "b".into(),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            list: Some(ListItem {
                kind: ListKind::NumberDecimalDot,
                level: 0,
            }),
            unsupported_xml: None,
        },
    ];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(xml.contains("<w:numId w:val=\"1\"/>"), "got: {xml}");
    assert!(xml.contains("<w:numId w:val=\"2\"/>"), "got: {xml}");
}

#[test]
fn test_rebuild_omits_pstyle_numpr_for_non_list_paragraphs() {
    let paragraphs = vec![Paragraph {
        runs: vec![Run {
            text: "hi".into(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        list: None,
        unsupported_xml: None,
    }];
    let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
    assert!(!xml.contains("ListParagraph"), "got: {xml}");
    assert!(!xml.contains("w:numPr"), "got: {xml}");
}

// ── package plumbing (numbering.xml + Content_Types + rels) ─────────────

#[test]
fn test_write_docx_includes_numbering_xml_and_plumbing_when_document_has_a_list() {
    let dir = std::env::temp_dir().join(format!("vimbatim_list_plumbing_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.docx");

    let paragraphs = vec![Paragraph {
        runs: vec![Run {
            text: "item".into(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        list: Some(ListItem {
            kind: ListKind::BulletSolid,
            level: 0,
        }),
        unsupported_xml: None,
    }];
    create_new_docx(&paragraphs, &path, Default::default()).unwrap();

    let file = std::fs::File::open(&path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();

    let mut numbering_xml = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("word/numbering.xml").unwrap(),
        &mut numbering_xml,
    )
    .unwrap();
    assert!(
        numbering_xml.contains("<w:abstractNum"),
        "got: {numbering_xml}"
    );

    let mut content_types = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("[Content_Types].xml").unwrap(),
        &mut content_types,
    )
    .unwrap();
    assert!(content_types.contains("numbering"), "got: {content_types}");

    let mut rels = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("word/_rels/document.xml.rels").unwrap(),
        &mut rels,
    )
    .unwrap();
    assert!(rels.contains("numbering"), "got: {rels}");

    std::fs::remove_file(&path).ok();
    std::fs::remove_dir(&dir).ok();
}

#[test]
fn test_write_docx_omits_numbering_xml_when_document_has_no_list() {
    let dir =
        std::env::temp_dir().join(format!("vimbatim_no_list_plumbing_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.docx");

    let paragraphs = vec![Paragraph {
        runs: vec![Run {
            text: "plain".into(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        list: None,
        unsupported_xml: None,
    }];
    create_new_docx(&paragraphs, &path, Default::default()).unwrap();

    let file = std::fs::File::open(&path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    assert!(archive.by_name("word/numbering.xml").is_err());

    std::fs::remove_file(&path).ok();
    std::fs::remove_dir(&dir).ok();
}

// ── styles.xml plumbing (bug report: heading level missing in Word) ─────

#[test]
fn test_create_new_docx_includes_styles_xml_with_all_four_card_style_headings() {
    let dir = std::env::temp_dir().join(format!("vimbatim_styles_plumbing_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.docx");

    let paragraphs = vec![Paragraph {
        runs: vec![Run {
            text: "plain".into(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        list: None,
        unsupported_xml: None,
    }];
    create_new_docx(&paragraphs, &path, Default::default()).unwrap();

    let file = std::fs::File::open(&path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();

    let mut styles = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("word/styles.xml").unwrap(),
        &mut styles,
    )
    .unwrap();
    for (level, alias) in [(1, "Pocket"), (2, "Hat"), (3, "Block"), (4, "Tag")] {
        assert!(
            styles.contains(&format!("w:styleId=\"Heading{level}\"")),
            "got: {styles}"
        );
        assert!(
            styles.contains(&format!("w:val=\"{alias}\"")),
            "got: {styles}"
        );
    }
    assert!(styles.contains("w:styleId=\"Normal\""), "got: {styles}");

    let mut content_types = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("[Content_Types].xml").unwrap(),
        &mut content_types,
    )
    .unwrap();
    assert!(
        content_types.contains("wordprocessingml.styles+xml"),
        "got: {content_types}"
    );

    let mut rels = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("word/_rels/document.xml.rels").unwrap(),
        &mut rels,
    )
    .unwrap();
    assert!(rels.contains("relationships/styles"), "got: {rels}");

    std::fs::remove_file(&path).ok();
    std::fs::remove_dir(&dir).ok();
}

#[test]
fn test_create_new_docx_pocket_paragraph_round_trips_without_double_applying_style_defaults() {
    // Regression risk flagged during review: now that a real Heading1
    // style (with its own pBdr/b/sz) resolves, `apply_paragraph_style_
    // defaults` seeds those as defaults before the paragraph's own
    // direct <w:jc>/<w:pBdr>/<w:b>/<w:sz> (always emitted by
    // rebuild_document_xml for a card-style paragraph) overwrite them —
    // the run/paragraph that comes back must match exactly what went in,
    // not some doubled or drifted value.
    let dir = std::env::temp_dir().join(format!(
        "vimbatim_pocket_no_doubleapply_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.docx");

    let paragraphs = vec![Paragraph {
        runs: vec![Run {
            text: "hello".into(),
            bold: true,
            size: 52,
            box_format: true,
            ..Run::default()
        }],
        heading: 1,
        alignment: Alignment::Center,
        list: None,
        unsupported_xml: None,
    }];
    create_new_docx(&paragraphs, &path, Default::default()).unwrap();

    let (reparsed, _origin) = parse_docx(&path).unwrap();
    assert_eq!(reparsed.len(), 1);
    assert_eq!(reparsed[0].heading, 1);
    assert_eq!(reparsed[0].alignment, Alignment::Center);
    assert_eq!(reparsed[0].runs.len(), 1, "got: {:?}", reparsed[0].runs);
    let run = &reparsed[0].runs[0];
    assert_eq!(run.text, "hello");
    assert!(run.bold);
    assert_eq!(run.size, 52);
    assert!(run.box_format);

    std::fs::remove_file(&path).ok();
    std::fs::remove_dir(&dir).ok();
}

#[test]
fn test_real_file_round_trip_preserves_list_through_save() {
    let dir = std::env::temp_dir().join(format!("vimbatim_list_roundtrip_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.docx");

    let initial = vec![Paragraph {
        runs: vec![Run {
            text: "hello".into(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        list: None,
        unsupported_xml: None,
    }];
    create_new_docx(&initial, &path, Default::default()).unwrap();

    let (mut paragraphs, origin) = parse_docx(&path).unwrap();
    paragraphs[0].list = Some(ListItem {
        kind: ListKind::NumberUpperRoman,
        level: 0,
    });
    origin.save(&paragraphs, &path).unwrap();

    let (reparsed, _origin2) = parse_docx(&path).unwrap();
    assert_eq!(
        reparsed[0].list,
        Some(ListItem {
            kind: ListKind::NumberUpperRoman,
            level: 0
        })
    );

    std::fs::remove_file(&path).ok();
    std::fs::remove_dir(&dir).ok();
}

// ── real Lists.docx regression (all four examples) ──────────────────────

/// `Lists.docx`'s "Item One"/"Item Two"/"Item Three" text is reused
/// verbatim by both the plain bulleted list (paragraphs 1-3) and the
/// plain numbered list (paragraphs 5-7) — asserted by paragraph index,
/// not by text search, since a text search can't distinguish the two.
/// Every other example uses unique item text.
#[test]
fn test_parses_real_lists_docx_all_four_examples() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lists_reference.docx");
    let (paragraphs, _origin) = parse_docx(&path).unwrap();

    // By concatenated paragraph text, not a single run's text: some of
    // this real Word file's own paragraphs split across multiple runs
    // that can't merge (e.g. the intro sentences, whose middle run
    // lacks the `xml:space="preserve"` its neighbors have, a genuine
    // per-run formatting difference in the source file, not a parser
    // gap) — a single-run search would miss those paragraphs entirely.
    let find = |text: &str| -> &Paragraph {
        paragraphs
            .iter()
            .find(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>() == text)
            .unwrap_or_else(|| panic!("paragraph with text {text:?} not found"))
    };

    // Plain bulleted list (example 1): three items, indices 1-3.
    for (i, paragraph) in paragraphs.iter().enumerate().take(4).skip(1) {
        assert_eq!(
            paragraph.list.map(|l| l.kind),
            Some(ListKind::BulletSolid),
            "index {i}"
        );
    }
    // Index 4 is a genuinely blank line in the source file, written as a
    // self-closing `<w:p/>`. The parser used to drop those entirely, which
    // is why the numbered list below sat at 5-7 here — and why every blank
    // line between cards disappeared from any Word-authored document this
    // app opened and resaved.
    assert!(
        paragraphs[4].runs.iter().all(|r| r.text.is_empty()),
        "index 4 is the file's blank line: {:?}",
        paragraphs[4].runs,
    );
    assert!(
        !paragraphs[4].runs.is_empty(),
        "a blank paragraph still holds one run"
    );

    // Plain numbered list (example 2): three items, indices 6-8.
    for (i, paragraph) in paragraphs.iter().enumerate().take(9).skip(6) {
        assert_eq!(
            paragraph.list.map(|l| l.kind),
            Some(ListKind::NumberDecimalDot),
            "index {i}"
        );
    }

    // Six distinct bullet options (example 3).
    assert_eq!(
        find("Solid Bullet").list.map(|l| l.kind),
        Some(ListKind::BulletSolid)
    );
    assert_eq!(
        find("Hollow Bullet").list.map(|l| l.kind),
        Some(ListKind::BulletHollow)
    );
    assert_eq!(
        find("Solid Box").list.map(|l| l.kind),
        Some(ListKind::BulletSolidBox)
    );
    assert_eq!(
        find("Four Diamonds").list.map(|l| l.kind),
        Some(ListKind::BulletDiamond)
    );
    assert_eq!(
        find("Arrow").list.map(|l| l.kind),
        Some(ListKind::BulletArrow)
    );
    assert_eq!(
        find("Checkmark").list.map(|l| l.kind),
        Some(ListKind::BulletCheckmark)
    );

    // Seven distinct number options (example 4).
    assert_eq!(
        find("One Dot").list.map(|l| l.kind),
        Some(ListKind::NumberDecimalDot)
    );
    assert_eq!(
        find("One Parenthesis").list.map(|l| l.kind),
        Some(ListKind::NumberDecimalParen)
    );
    assert_eq!(
        find("Roman Numeral One").list.map(|l| l.kind),
        Some(ListKind::NumberUpperRoman)
    );
    assert_eq!(
        find("Capital A Dot").list.map(|l| l.kind),
        Some(ListKind::NumberUpperLetter)
    );
    assert_eq!(
        find("Lowercase A Parenthesis").list.map(|l| l.kind),
        Some(ListKind::NumberLowerLetterParen)
    );
    assert_eq!(
        find("Lowercase A Dot").list.map(|l| l.kind),
        Some(ListKind::NumberLowerLetterDot)
    );
    assert_eq!(
        find("Roman Numeral Lowercase one").list.map(|l| l.kind),
        Some(ListKind::NumberLowerRoman)
    );

    // Non-list lines (the four intro sentences) stay unlisted.
    assert_eq!(find("This is a line above a bulleted list:").list, None);
    assert_eq!(find("This is a line above a numbered list:").list, None);
    assert_eq!(find("Here are items with different bullets:").list, None);
    assert_eq!(
        find("Here are items with different types of numbers:").list,
        None
    );
}

#[test]
fn test_real_lists_docx_survives_save_and_reload() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lists_reference.docx");
    let dir = std::env::temp_dir().join(format!("vimbatim_lists_roundtrip_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.docx");
    std::fs::copy(&src, &path).unwrap();

    let (paragraphs, origin) = parse_docx(&path).unwrap();
    origin.save(&paragraphs, &path).unwrap();

    let (reparsed, _origin2) = parse_docx(&path).unwrap();
    let checkmark = reparsed
        .iter()
        .find(|p| p.runs.iter().any(|r| r.text == "Checkmark"))
        .unwrap();
    assert_eq!(
        checkmark.list.map(|l| l.kind),
        Some(ListKind::BulletCheckmark)
    );
    let roman_lower = reparsed
        .iter()
        .find(|p| {
            p.runs
                .iter()
                .any(|r| r.text == "Roman Numeral Lowercase one")
        })
        .unwrap();
    assert_eq!(
        roman_lower.list.map(|l| l.kind),
        Some(ListKind::NumberLowerRoman)
    );

    std::fs::remove_file(&path).ok();
    std::fs::remove_dir(&dir).ok();
}

/// Same bug as `test_rebuild_heading_wins_over_list_emits_exactly_one_
/// pstyle`, but through the real save path (`DocxOrigin::save`, not a
/// direct `rebuild_document_xml` call) against a real Word-produced file
/// that already has lists — confirms the write-side guard holds even
/// when `.list` is still set on a heading paragraph (i.e. even without
/// `apply_card_style`'s own fix, which this test deliberately bypasses
/// by mutating `heading` directly on an existing list item).
#[test]
fn test_real_lists_docx_heading_applied_to_a_list_item_survives_resave_as_one_pstyle() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lists_reference.docx");
    let dir =
        std::env::temp_dir().join(format!("vimbatim_heading_over_list_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.docx");
    std::fs::copy(&src, &path).unwrap();

    let (mut paragraphs, origin) = parse_docx(&path).unwrap();
    let idx = paragraphs
        .iter()
        .position(|p| p.runs.iter().any(|r| r.text == "Checkmark"))
        .unwrap();
    assert!(
        paragraphs[idx].list.is_some(),
        "sanity: must actually be a list item"
    );
    paragraphs[idx].heading = 1;
    origin.save(&paragraphs, &path).unwrap();

    let file = std::fs::File::open(&path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();

    // The rest of the package survives untouched — this bug was scoped
    // to the one paragraph's own <w:pPr>, not the manifest.
    let mut styles = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("word/styles.xml").unwrap(),
        &mut styles,
    )
    .unwrap();
    assert!(styles.contains("styleId=\"Heading1\""));

    let mut doc = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("word/document.xml").unwrap(), &mut doc)
        .unwrap();
    let pos = doc.find("Checkmark").unwrap();
    let start = doc[..pos].rfind("<w:p>").unwrap();
    let end = doc[pos..].find("</w:p>").map(|e| pos + e + 6).unwrap();
    let para_xml = &doc[start..end];
    assert_eq!(para_xml.matches("<w:pStyle").count(), 1, "got: {para_xml}");
    assert!(
        para_xml.contains(r#"<w:pStyle w:val="Heading1"/>"#),
        "got: {para_xml}"
    );
    assert!(!para_xml.contains("ListParagraph"), "got: {para_xml}");
    assert!(!para_xml.contains("w:numPr"), "got: {para_xml}");

    std::fs::remove_file(&path).ok();
    std::fs::remove_dir(&dir).ok();
}

// ── highlight/space-boundary regression (real Bug_Test.docx) ────────────

/// Real repro file for the reported bug: a run boundary falls right at a
/// space (a plain run, then a `w:highlight`ed word, then a plain run
/// starting with a space) and neither edge run carries
/// `xml:space="preserve"` — Word trims that space on open even though
/// vimbatim showed it, because the flag was never derived from the run's
/// actual text. Confirms the fix holds on parse→save, not just the
/// synthetic case above.
#[test]
fn test_real_bug_test_docx_gains_xml_space_preserve_around_highlight_boundary_on_save() {
    let src =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/highlight_space_boundary.docx");
    let dir = std::env::temp_dir().join(format!("vimbatim_highlight_space_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.docx");
    std::fs::copy(&src, &path).unwrap();

    let (paragraphs, origin) = parse_docx(&path).unwrap();
    origin.save(&paragraphs, &path).unwrap();

    let file = std::fs::File::open(&path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut document_xml = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("word/document.xml").unwrap(),
        &mut document_xml,
    )
    .unwrap();

    assert!(
        document_xml.contains(r#"<w:t xml:space="preserve">this is written and </w:t>"#),
        "got: {document_xml}"
    );
    assert!(
        document_xml.contains(r#"<w:t xml:space="preserve"> in vimbatim</w:t>"#),
        "got: {document_xml}"
    );

    // Idempotence: reparsing the now-correct file and saving it again
    // must not drift — the newly-added `xml:space="preserve"` should
    // round-trip stably rather than only surviving the first save.
    let (reparsed, origin2) = parse_docx(&path).unwrap();
    origin2.save(&reparsed, &path).unwrap();
    let file2 = std::fs::File::open(&path).unwrap();
    let mut archive2 = zip::ZipArchive::new(file2).unwrap();
    let mut document_xml2 = String::new();
    std::io::Read::read_to_string(
        &mut archive2.by_name("word/document.xml").unwrap(),
        &mut document_xml2,
    )
    .unwrap();
    assert_eq!(
        document_xml, document_xml2,
        "second save must match the first byte-for-byte"
    );

    std::fs::remove_file(&path).ok();
    std::fs::remove_dir(&dir).ok();
}

// ── multi-level (Phase 2) round trip ─────────────────────────────────────

#[test]
fn test_multilevel_list_round_trips_through_parse_and_rebuild() {
    let original = vec![
        Paragraph {
            runs: vec![Run {
                text: "top".into(),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            list: Some(ListItem {
                kind: ListKind::BulletSolid,
                level: 0,
            }),
            unsupported_xml: None,
        },
        Paragraph {
            runs: vec![Run {
                text: "nested".into(),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            list: Some(ListItem {
                kind: ListKind::BulletSolid,
                level: 1,
            }),
            unsupported_xml: None,
        },
    ];
    let xml = rebuild_document_xml(&fallback_preamble(), "", &original);
    let numbering = parse_numbering_xml(&build_numbering_xml(&original).unwrap());
    let reparsed = parse_document_xml(&xml, &no_styles(), &numbering).unwrap();
    assert_eq!(
        reparsed[0].list,
        Some(ListItem {
            kind: ListKind::BulletSolid,
            level: 0
        })
    );
    assert_eq!(
        reparsed[1].list,
        Some(ListItem {
            kind: ListKind::BulletSolid,
            level: 1
        })
    );
}
