#![doc = include_str!("../README.md")]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

macro_rules! sol {
    ($($input:tt)*) => {
        #[cfg(feature = "serde")]
        alloy_sol_types::sol! {
            #[derive(serde::Serialize, serde::Deserialize)]
            $($input)*
        }
        #[cfg(not(feature = "serde"))]
        alloy_sol_types::sol! {
            $($input)*
        }
    };
}

pub(crate) use sol;

mod precompiles;
pub use precompiles::*;
