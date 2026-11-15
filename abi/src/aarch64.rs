//! Values that the ARM64 architecture layer fixes (Unlike cores).

use crate::FeatureList;

/// State features: SVE and SME have a length; SME is large; ZT0 of SME2.
pub const SVE: u32 = 0;
pub const SME: u32 = 1;
pub const SME2_ZT0: u32 = 2;
pub const FEATURES: FeatureList = FeatureList {
    state: 1 << SVE | 1 << SME | 1 << SME2_ZT0,
    with_length: 1 << SVE | 1 << SME,
    large: 1 << SME,
};
