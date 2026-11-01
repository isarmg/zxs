use erased::ErasedBox;

fn check(id_1: &std::any::TypeId, id_2: &std::any::TypeId) {
    if id_1 != id_2 {
        panic!("Erased: type mismatch")
    }
}

macro_rules! erased {
    ([$($new_t_params:tt)*], $name:ident) => {
        /// A type-erased item that verifies the requested type before retrieval.
        pub struct $name {
            type_id: std::any::TypeId,
            value: Option<ErasedBox>,
            drop: fn(ErasedBox),
        }


        impl $name {
            /// Create a new type-erased item.
            pub fn new<T: $($new_t_params)*>(item: T) -> Self {
                Self {
                    type_id: std::any::TypeId::of::<T>(),
                    value: Some(ErasedBox::new(Box::new(item))),
                    drop: |value| {
                        // SAFETY: This destructor is stored with the same Box<T>
                        // created above and cannot be replaced by callers.
                        let _ = unsafe { value.into_inner::<T>() };
                    },
                }
            }

            /// Get a reference to the inner value.
            ///
            /// Panics if `T` is not the stored type.
            pub fn get_ref<T: 'static>(&self) -> &T {
                check(&self.type_id, &std::any::TypeId::of::<T>());
                // SAFETY: The TypeId check proves the stored Box<T> type, and
                // the shared borrow of self keeps it alive for this reference.
                unsafe { self.value.as_ref().unwrap().get_ref::<T>() }
            }

            /// Get a mutable reference to the inner value.
            ///
            /// Panics if `T` is not the stored type.
            pub fn get_mut<T: 'static>(&mut self) -> &mut T {
                check(&self.type_id, &std::any::TypeId::of::<T>());
                // SAFETY: The TypeId check proves the stored Box<T> type, and
                // the exclusive borrow of self prevents aliasing this value.
                unsafe { self.value.as_mut().unwrap().get_mut::<T>() }
            }

            /// Consume the item and return the inner value.
            ///
            /// Panics if `T` is not the stored type.
            pub fn into_inner<T: 'static>(mut self) -> T {
                check(&self.type_id, &std::any::TypeId::of::<T>());
                // SAFETY: The TypeId check proves the stored Box<T> type, and
                // taking its ownership prevents Drop from destroying it twice.
                *unsafe { self.value.take().unwrap().into_inner::<T>() }
            }
        }

        /// If into_inner() wasn't called, the value would leak and destructors wouldn't run, this prevents that from happening.
        impl Drop for $name {
            fn drop(&mut self) {
                if let Some(value) = self.value.take() {
                    (self.drop)(value);
                }
            }
        }
    };

}

erased!([Send + 'static], Erased);
erased!(['static], ErasedLocal);

/// SAFETY: `Erased::new` ensures that `T` is `Send` and `'static`.
unsafe impl Send for Erased {}
