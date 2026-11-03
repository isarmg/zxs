#![doc(html_root_url = "https://docs.rs/ipnet/2.12.2")]
//! Types for IPv4 and IPv6 network addresses.
//!
//! This module provides types and useful methods for working with IPv4
//! and IPv6 network addresses, commonly called IP prefixes. The new
//! [`IpNet`], [`Ipv4Net`], and [`Ipv6Net`] types build on the existing
//! [`IpAddr`], [`Ipv4Addr`], and [`Ipv6Addr`] types already provided in
//! Rust's standard library and align to their design to stay
//! consistent.
//!
//! The module also provides the [`IpSubnets`], [`Ipv4Subnets`], and
//! [`Ipv6Subnets`] types for iterating over the subnets contained in
//! an IP address range. The [`IpAddrRange`], [`Ipv4AddrRange`], and
//! [`Ipv6AddrRange`] types for iterating over IP addresses in a range.
//! And traits that extend `Ipv4Addr` and `Ipv6Addr` with methods for
//! addition, subtraction, bitwise-and, and bitwise-or operations that
//! are missing in Rust's standard library.
//!
//! The module only uses stable features so it is guaranteed to compile
//! using the stable toolchain.
//!
//! # Organization
//!
//! * [`IpNet`] represents an IP network address, either IPv4 or IPv6.
//! * [`Ipv4Net`] and [`Ipv6Net`] are respectively IPv4 and IPv6 network
//!   addresses.
//! * [`IpSubnets`], [`Ipv4Subnets`], and [`Ipv6Subnets`] are iterators
//!   that generate the smallest set of IP network addresses bound by an
//!   IP address range and minimum prefix length. These can be created
//!   using their constructors. They are also returned by the
//!   [`subnets()`] methods and used within the [`aggregate()`] methods.
//! * [`IpAddrRange`], [`Ipv4AddrRange`], and [`Ipv6AddrRange`] are
//!   iterators that generate IP addresses. These can be created using
//!   their constructors. They are also returned by the [`hosts()`]
//!   methods.
//! * The [`IpAdd`], [`IpSub`], [`IpBitAnd`], [`IpBitOr`] traits extend
//!   the [`Ipv4Addr`] and [`Ipv6Addr`] types with methods to perform
//!   these operations.
//!
//! [`IpNet`]: enum.IpNet.html
//! [`Ipv4Net`]: struct.Ipv4Net.html
//! [`Ipv6Net`]: struct.Ipv6Net.html
//! [`IpAddr`]: https://doc.rust-lang.org/std/net/enum.IpAddr.html
//! [`Ipv4Addr`]: https://doc.rust-lang.org/std/net/struct.Ipv4Addr.html
//! [`Ipv6Addr`]: https://doc.rust-lang.org/std/net/struct.Ipv6Addr.html
//! [`IpSubnets`]: enum.IpSubnets.html
//! [`Ipv4Subnets`]: struct.Ipv4Subnets.html
//! [`Ipv6Subnets`]: struct.Ipv6Subnets.html
//! [`subnets()`]: enum.IpNet.html#method.subnets
//! [`aggregate()`]: enum.IpNet.html#method.aggregate
//! [`IpAddrRange`]: enum.IpAddrRange.html
//! [`Ipv4AddrRange`]: struct.Ipv4AddrRange.html
//! [`Ipv6AddrRange`]: struct.Ipv6AddrRange.html
//! [`hosts()`]: enum.IpNet.html#method.hosts
//! [`IpAdd`]: trait.IpAdd.html
//! [`IpSub`]: trait.IpSub.html
//! [`IpBitAnd`]: trait.IpBitAnd.html
//! [`IpBitOr`]: trait.IpBitOr.html
//!
//! # Features
//!
//! These flags can be used to extend functionality using third-party
//! dependencies or optional libraries. See the [features] reference
//! for more information.
//!
//! [features]: https://doc.rust-lang.org/cargo/reference/features.html#the-features-section
//!
//! ## "std"
//!
//! Enabled by default. Disabling this feature will mandate the use of the
//! [core] and [alloc] crates where applicable instead of [std].
//!
//! [core]: https://doc.rust-lang.org/core/
//! [alloc]: https://doc.rust-lang.org/alloc/
//! [std]: https://doc.rust-lang.org/std/
//!
//! ## "serialization"
//!
//! Uses the local `data-serialization` component to implement the
//! `Serialize` and `Deserialize` traits.
//!
//! For human readable formats (e.g. JSON) the `IpNet`, `Ipv4Net`, and
//! `Ipv6Net` types will serialize to their `Display` strings.
//!
//! For non-human-readable formats, `Ipv4Net` and `Ipv6Net` serialize
//! as tuples of 5 and 17 `u8` elements: address octets followed by the
//! prefix length. `IpNet` uses a newtype enum variant, `V4` (index 0)
//! or `V6` (index 1), containing the corresponding tuple. The format
//! determines the byte encoding of tuples and enum discriminants;
//! these data-model shapes do not guarantee a particular byte layout.
//!
//! Deserialization parses the display string or validates the tuple's
//! prefix length with the corresponding network constructor.
//!
//! ## Provider-backed feature limitations
//!
//! The local manifest still declares `heapless`, `ser_as_str`,
//! `schemars08`, `schemars1`, `json`, and `schemars`, but does not supply
//! the `heapless` or schema-provider dependencies. Their provider-backed
//! behavior requires a separate feature-design decision.
//!
//! `heapless` currently enables only `serialization`; it neither enables
//! `ser_as_str` nor guarantees allocation-free formatting. Without
//! `ser_as_str`, human-readable serialization delegates to the format's
//! `Serializer::collect_str` implementation.
//!
//! `ser_as_str` alone does not enable `serialization`. Combining them
//! selects code that requires the absent `heapless` provider and cannot
//! currently compile.
//!
//! `schemars08` and `schemars1` require their absent schema providers.
//! `schemars` enables `schemars08`, while `json` enables `schemars08`
//! and `serialization`; these combinations cannot currently compile
//! either. No schema support or provider replacement is implied by
//! the declared flags.
//!

#![no_std]

#[cfg(feature = "std")]
extern crate std;

#[cfg_attr(test, macro_use)]
extern crate alloc;

#[cfg(feature = "schemars08")]
extern crate schemars08;
#[cfg(feature = "schemars1")]
extern crate schemars1;
#[cfg(feature = "serialization")]
extern crate data_serialization;

pub use self::ipext::{IpAdd, IpAddrRange, IpBitAnd, IpBitOr, IpSub, Ipv4AddrRange, Ipv6AddrRange};
pub use self::ipnet::{
    IpNet, IpSubnets, Ipv4Net, Ipv4Subnets, Ipv6Net, Ipv6Subnets, PrefixLenError,
};
pub use self::mask::{ip_mask_to_prefix, ipv4_mask_to_prefix, ipv6_mask_to_prefix};
pub use self::parser::AddrParseError;

mod ipext;
mod ipnet;
mod mask;
mod parser;

#[cfg(feature = "schemars08")]
mod ipnet_schemars_08;
#[cfg(feature = "schemars1")]
mod ipnet_schemars_1;
#[cfg(feature = "serialization")]
mod ipnet_serialization;
