//! #4736: an oversized cantSplit row moves once, then retains its text across pages.
use super::*;

/// Every source line is placed once, wholly inside its page's body rather than below it.
fn assert_complete_row(layout: &DocLayout, block: u32, count: usize) -> Vec<usize> {
    let mut seen = std::collections::BTreeSet::new();
    let mut pages = Vec::new();
    let mut total = 0;
    for (page_index, page) in layout.pages.iter().enumerate() {
        for item in &page.items {
            let Placed::Lines { story: StoryRef::Body, path, para, l0, l1, y, .. } = item else { continue };
            if path.0.len() != 4 || path.0[..3] != [block, 0, 0] {
                continue;
            }
            assert!(seen.insert(path.0[3]), "a short source paragraph is drawn only once");
            total += l1 - l0;
            assert!(*y >= page.body.y - 0.02, "paragraph starts above the body: {y}");
            let bottom = item_bottom(*y, para, *l0, *l1).unwrap();
            assert!(bottom <= page.body.bottom() + 0.02, "paragraph {} runs below page {}: {bottom} > {}", path.0[3], page_index, page.body.bottom());
            if pages.last() != Some(&page_index) {
                pages.push(page_index);
            }
        }
    }
    assert_eq!(seen.into_iter().collect::<Vec<_>>(), (0..count as u32).collect::<Vec<_>>());
    assert_eq!(total, count, "no source line is lost or repeated");
    pages
}

/// Word for Mac 16.114.2 starts the oversized row on a fresh page, then continues it.
#[test]
fn oversized_cant_split_row_moves_once_then_flows() {
    for minimum in [false, true] {
        let (layout, rows) = at_least_rows(200.0, 1, |table| {
            table.rows[0].props.cant_split = true;
            table.rows[0].cells[0].blocks = lines_in_cell(80, "Synthetic");
            if minimum {
                table.rows[0].props.height = Some(12.0);
                table.rows[0].props.height_rule = HeightRule::AtLeast;
            }
        });
        assert_eq!(rows[0], Vec::<usize>::new(), "cantSplit still moves off the partly filled page");
        let pages = assert_complete_row(&layout, 1, 80);
        assert_eq!(pages.first(), Some(&1));
        assert!(pages.len() >= 2, "a row taller than a page must continue");
    }
}

/// At the top of a fresh page there is no benefit in advancing again before splitting.
#[test]
fn oversized_cant_split_row_at_page_top_flows_without_a_blank_page() {
    let (layout, _) = at_least_rows(648.0, 1, |table| {
        table.rows[0].props.cant_split = true;
        table.rows[0].cells[0].blocks = lines_in_cell(80, "Synthetic");
    });
    let pages = assert_complete_row(&layout, 0, 80);
    assert_eq!(pages.first(), Some(&0));
    assert!(pages.len() >= 2);
}

/// The exception does not split a cantSplit row that fits on a fresh page.
#[test]
fn fitting_cant_split_row_still_moves_whole() {
    let (layout, rows) = at_least_rows(200.0, 1, |table| {
        table.rows[0].props.cant_split = true;
        table.rows[0].cells[0].blocks = lines_in_cell(20, "Synthetic");
    });
    assert_eq!(rows, vec![vec![], vec![0]]);
    assert_eq!(assert_complete_row(&layout, 1, 20), vec![1]);
}

/// Exact heights retain their explicit clipping contract, even with cantSplit present.
#[test]
fn exact_height_cant_split_row_is_not_split_to_reveal_overflow() {
    let (layout, rows) = at_least_rows(648.0, 1, |table| {
        let row = &mut table.rows[0];
        row.props.cant_split = true;
        row.props.height = Some(240.0);
        row.props.height_rule = HeightRule::Exact;
        row.cells[0].blocks = lines_in_cell(80, "Synthetic");
    });
    assert_eq!(rows, vec![vec![0]]);
    assert!(layout.pages[0].items.iter().any(|item| matches!(item, Placed::Cell { table, row: 0, clip: true, .. } if table.0 == [0])));
}
