//! Sixth review (Mistral F4), turned to assert the contract: `Geometry::valid`
//! ties *E* to the page, so a 64 KB granule (8,192 entries per table) is
//! refused whatever E is stated, and so is an E that is not the entries of
//! one page.
use dodos_abi::{Geometry, TABLE_ENTRIES_MAX};

#[test]
fn a_sixty_four_kb_granule_is_refused() {
    let g = Geometry {
        page_shift: [16, 29, 0, 0],
        entries: 4096,
        levels: 3,
        user_end: 63 << 42,
    };
    assert!(!g.valid(), "a 64 KB page with E = 4096");
    assert!(
        !Geometry { entries: 8192, ..g }.valid(),
        "E above the maximum"
    );
    assert_eq!(TABLE_ENTRIES_MAX, 4096);
    assert_eq!(1u64 << 16 >> 3, 8192, "a 64 KB table page has 8192 entries");
}

#[test]
fn entries_must_be_those_of_one_page() {
    let g = Geometry {
        page_shift: [12, 21, 30, 0],
        entries: 2,
        levels: 4,
        user_end: 1 << 30,
    };
    assert!(!g.valid(), "E = 2 on a 4 KB page");
    assert!(Geometry { entries: 512, ..g }.valid());
}
