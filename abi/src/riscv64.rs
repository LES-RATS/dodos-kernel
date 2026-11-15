//! Values that the RISC-V architecture layer fixes (Unlike cores).

use crate::FeatureList;

/// State features: the vector extension, which has a length.
pub const V: u32 = 0;
pub const FEATURES: FeatureList = FeatureList {
    state: 1 << V,
    with_length: 1 << V,
    large: 0,
};
