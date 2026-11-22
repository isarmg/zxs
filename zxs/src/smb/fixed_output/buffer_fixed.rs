// ZXS adaptation of RustCrypto fixed-output support; see ORIGIN.md.
macro_rules! buffer_fixed {
    (
        $(#[$attr:meta])*
        $v:vis struct $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        impl: $($trait_name:ident)*;
    ) => {
        $(#[$attr])*
        $v struct $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? {
            core: $core_ty,
            buffer: $crate::smb::fixed_output::block_api::Buffer<$core_ty>,
        }

        $crate::smb::fixed_output::buffer_fixed!(
            impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty);
            $($trait_name)*;
        );
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        ;
    ) => {};

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        MacTraits $($trait_name:ident)*;
    ) => {
        $crate::smb::fixed_output::buffer_fixed!(
            impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty);
            BaseFixedTraits MacMarker $($trait_name)*;
        );
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        ResetMacTraits $($trait_name:ident)*;
    ) => {
        $crate::smb::fixed_output::buffer_fixed!(
            impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty);
            MacTraits Reset FixedOutputReset $($trait_name)*;
        );
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        BaseFixedTraits $($trait_name:ident)*;
    ) => {
        $crate::smb::fixed_output::buffer_fixed!(
            impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty);
            Debug BlockSizeUser OutputSizeUser CoreProxy Update FixedOutput $($trait_name)*;
        );
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        Debug $($trait_name:ident)*;
    ) => {
        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? core::fmt::Debug for $name$(< $( $lt ),+ >)? {
            #[inline]
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str(concat!(stringify!($name), " { ... }"))
            }
        }

        $crate::smb::fixed_output::buffer_fixed!(impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty); $($trait_name)*;);
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        AlgorithmName $($trait_name:ident)*;
    ) => {
        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $crate::smb::fixed_output::common::AlgorithmName for $name$(< $( $lt ),+ >)? {
            #[inline]
            fn write_alg_name(f: &mut core::fmt::Formatter<'_>) -> Result<(), core::fmt::Error> {
                <$core_ty as $crate::smb::fixed_output::common::AlgorithmName>::write_alg_name(f)
            }
        }

        $crate::smb::fixed_output::buffer_fixed!(impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty); $($trait_name)*;);
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        BlockSizeUser $($trait_name:ident)*;
    ) => {
        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $crate::smb::fixed_output::block_api::BlockSizeUser for $name$(< $( $lt ),+ >)? {
            type BlockSize = <$core_ty as $crate::smb::fixed_output::common::BlockSizeUser>::BlockSize;
        }

        $crate::smb::fixed_output::buffer_fixed!(impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty); $($trait_name)*;);
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        OutputSizeUser $($trait_name:ident)*;
    ) => {
        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $crate::smb::fixed_output::OutputSizeUser for $name$(< $( $lt ),+ >)? {
            type OutputSize = <$core_ty as $crate::smb::fixed_output::block_api::OutputSizeUser>::OutputSize;
        }

        $crate::smb::fixed_output::buffer_fixed!(impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty); $($trait_name)*;);
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        CoreProxy $($trait_name:ident)*;
    ) => {
        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $crate::smb::fixed_output::block_api::CoreProxy for $name$(< $( $lt ),+ >)? {
            type Core = $core_ty;

        }

        $crate::smb::fixed_output::buffer_fixed!(impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty); $($trait_name)*;);
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        Update $($trait_name:ident)*;
    ) => {
        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $crate::smb::fixed_output::Update for $name$(< $( $lt ),+ >)? {
            #[inline]
            fn update(&mut self, data: &[u8]) {
                let Self { core, buffer } = self;
                buffer.digest_blocks(data, |blocks| {
                    $crate::smb::fixed_output::block_api::UpdateCore::update_blocks(core, blocks)
                });
            }
        }

        $crate::smb::fixed_output::buffer_fixed!(impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty); $($trait_name)*;);
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        FixedOutput $($trait_name:ident)*;
    ) => {
        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $crate::smb::fixed_output::FixedOutput for $name$(< $( $lt ),+ >)? {
            #[inline]
            fn finalize_into(mut self, out: &mut $crate::smb::fixed_output::Output<Self>) {
                let Self { core, buffer } = &mut self;
                $crate::smb::fixed_output::block_api::FixedOutputCore::finalize_fixed_core(core, buffer, out);
            }
        }

        $crate::smb::fixed_output::buffer_fixed!(impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty); $($trait_name)*;);
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        Default $($trait_name:ident)*;
    ) => {
        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? Default for $name$(< $( $lt ),+ >)? {
            #[inline]
            fn default() -> Self {
                Self {
                    core: Default::default(),
                    buffer: Default::default(),
                }
            }
        }

        $crate::smb::fixed_output::buffer_fixed!(impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty); $($trait_name)*;);
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        Clone $($trait_name:ident)*;
    ) => {
        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? Clone for $name$(< $( $lt ),+ >)? {
            #[inline]
            fn clone(&self) -> Self {
                Self {
                    core: Clone::clone(&self.core),
                    buffer: Clone::clone(&self.buffer),
                }
            }
        }

        $crate::smb::fixed_output::buffer_fixed!(impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty); $($trait_name)*;);
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        HashMarker $($trait_name:ident)*;
    ) => {
        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $crate::smb::fixed_output::HashMarker for $name$(< $( $lt ),+ >)? {}

        // Verify that `$core_ty` implements `HashMarker`
        const _: () = {
            fn check$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?(v: &$core_ty) {
                v as &dyn $crate::smb::fixed_output::HashMarker;
            }
        };

        $crate::smb::fixed_output::buffer_fixed!(impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty); $($trait_name)*;);
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        MacMarker $($trait_name:ident)*;
    ) => {
        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $crate::smb::fixed_output::MacMarker for $name$(< $( $lt ),+ >)? {}

        // Verify that `$core_ty` implements `MacMarker`
        const _: () = {
            fn check$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?(v: &$core_ty) {
                v as &dyn $crate::smb::fixed_output::MacMarker;
            }
        };

        $crate::smb::fixed_output::buffer_fixed!(impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty); $($trait_name)*;);
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        InnerInit $($trait_name:ident)*;
    ) => {
        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $crate::smb::fixed_output::common::InnerUser for $name$(< $( $lt ),+ >)? {
            type Inner = <$core_ty as $crate::smb::fixed_output::common::InnerUser>::Inner;
        }

        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $crate::smb::fixed_output::common::InnerInit for $name$(< $( $lt ),+ >)? {
            #[inline]
            fn inner_init(inner: Self::Inner) -> Self {
                Self {
                    core: <$core_ty as $crate::smb::fixed_output::common::InnerInit>::inner_init(inner),
                    buffer: Default::default(),
                }
            }
        }

        $crate::smb::fixed_output::buffer_fixed!(impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty); $($trait_name)*;);
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        KeyInit $($trait_name:ident)*;
    ) => {
        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $crate::smb::fixed_output::common::KeySizeUser for $name$(< $( $lt ),+ >)? {
            type KeySize = <$core_ty as $crate::smb::fixed_output::common::KeySizeUser>::KeySize;
        }

        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $crate::smb::fixed_output::KeyInit for $name$(< $( $lt ),+ >)? {
            #[inline]
            fn new(key: &$crate::smb::fixed_output::Key<Self>) -> Self {
                Self {
                    core: <$core_ty as $crate::smb::fixed_output::KeyInit>::new(key),
                    buffer: Default::default(),
                }
            }

            #[inline]
            fn new_from_slice(key: &[u8]) -> Result<Self, $crate::smb::fixed_output::InvalidLength> {
                <$core_ty as $crate::smb::fixed_output::KeyInit>::new_from_slice(key).map(|core|
                    Self { core, buffer: Default::default() }
                )
            }
        }

        $crate::smb::fixed_output::buffer_fixed!(impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty); $($trait_name)*;);
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        Reset $($trait_name:ident)*;
    ) => {
        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $crate::smb::fixed_output::Reset for $name$(< $( $lt ),+ >)? {
            #[inline]
            fn reset(&mut self) {
                $crate::smb::fixed_output::Reset::reset(&mut self.core);
                self.buffer.reset();
            }
        }

        $crate::smb::fixed_output::buffer_fixed!(impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty); $($trait_name)*;);
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        FixedOutputReset $($trait_name:ident)*;
    ) => {
        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $crate::smb::fixed_output::FixedOutputReset for $name$(< $( $lt ),+ >)? {
            #[inline]
            fn finalize_into_reset(&mut self, out: &mut $crate::smb::fixed_output::Output<Self>) {
                let Self { core, buffer } = self;
                $crate::smb::fixed_output::block_api::FixedOutputCore::finalize_fixed_core(core, buffer, out);
                $crate::smb::fixed_output::Reset::reset(self);
            }
        }

        $crate::smb::fixed_output::buffer_fixed!(impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty); $($trait_name)*;);
    };

    (
        impl_inner: $name:ident
        $(< $( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+ >)?
        ($core_ty:ty);
        SerializableState $($trait_name:ident)*;
    ) => {
        impl$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $crate::smb::fixed_output::common::state::SerializableState for $name$(< $( $lt ),+ >)? {
            type SerializedStateSize = $crate::smb::fixed_output::typenum::Sum<
                <$core_ty as $crate::smb::fixed_output::common::state::SerializableState>::SerializedStateSize,
                $crate::smb::fixed_output::block_buffer::SerializedBufferSize<
                    <$core_ty as $crate::smb::fixed_output::block_api::BlockSizeUser>::BlockSize,
                    <$core_ty as $crate::smb::fixed_output::block_api::BufferKindUser>::BufferKind,
                >
            >;

            #[inline]
            fn serialize(&self) -> $crate::smb::fixed_output::common::state::SerializedState<Self> {
                let serialized_core = self.core.serialize();
                let serialized_buf = self.buffer.serialize();
                serialized_core.concat(serialized_buf)
            }

            #[inline]
            fn deserialize(
                serialized_state: &$crate::smb::fixed_output::common::state::SerializedState<Self>,
            ) -> Result<Self, $crate::smb::fixed_output::common::state::DeserializeStateError> {
                use $crate::smb::fixed_output::common::state::{SerializableState, DeserializeStateError};

                let (serialized_core, serialized_buf) = serialized_state
                    .split_ref::<<$core_ty as SerializableState>::SerializedStateSize>();

                let core = SerializableState::deserialize(serialized_core)?;
                let buffer = $crate::smb::fixed_output::block_buffer::BlockBuffer::deserialize(serialized_buf)
                    .map_err(|_| DeserializeStateError)?;

                Ok(Self { core, buffer })
            }
        }

        $crate::smb::fixed_output::buffer_fixed!(impl_inner: $name$(< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)?($core_ty); $($trait_name)*;);
    };
}

pub(in crate::smb) use buffer_fixed;
