use super::*;

/// An iterator over the keys of a map.
#[derive(Debug, Clone, Copy)]
pub struct Keys<'map, 'elem: 'map> {
    map: &'map Map<'elem>,
    count: usize,
    index: usize,
}

impl<'map, 'elem> Keys<'map, 'elem> {
    #[inline]
    pub(crate) fn new(map: &'map Map<'elem>) -> Self {
        Self {
            map,
            count: map.key_count(),
            index: 0,
        }
    }
}

impl<'map, 'elem> Iterator for Keys<'map, 'elem> {
    type Item = &'map str;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.index == self.count {
            return None;
        }

        let key = self.map.key(self.index);
        self.index += 1;
        Some(key)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.count - self.index;
        (len, Some(len))
    }
}

impl<'map, 'elem> ExactSizeIterator for Keys<'map, 'elem> {}

/// An iterator over the values associated with a certain key of a map.
#[derive(Debug, Clone)]
pub struct ValueIter<'map, 'elem: 'map, T> {
    map: &'map Map<'elem>,
    key: CString,
    count: i32,
    index: i32,
    _variance: PhantomData<fn() -> T>,
}

macro_rules! impl_value_iter {
    ($value_type:path, $type:ty, $func:ident) => {
        impl<'map, 'elem> $crate::map::ValueIter<'map, 'elem, $type> {
            /// Creates a `ValueIter` from the given `map` and `key`.
            ///
            /// # Safety
            /// The caller must ensure `key` is valid.
            #[inline]
            pub(crate) unsafe fn new(
                map: &'map $crate::prelude::Map<'elem>,
                key: CString,
            ) -> $crate::map::Result<Self> {
                // Check if the value type is correct.
                match map.value_type_raw_unchecked(&key)? {
                    $value_type => {}
                    _ => return Err($crate::map::Error::WrongValueType),
                };

                let count = map.value_count_raw_unchecked(&key)? as i32;
                Ok(Self {
                    map,
                    key,
                    count,
                    index: 0,
                    _variance: std::marker::PhantomData,
                })
            }
        }

        impl<'map, 'elem> std::iter::Iterator for $crate::map::ValueIter<'map, 'elem, $type> {
            type Item = $type;

            #[inline]
            fn next(&mut self) -> Option<Self::Item> {
                if self.index == self.count {
                    return None;
                }

                let value = unsafe { self.map.$func(&self.key, self.index).unwrap() };
                self.index += 1;

                Some(value)
            }

            #[inline]
            fn size_hint(&self) -> (usize, Option<usize>) {
                let len = (self.count - self.index) as usize;
                (len, Some(len))
            }
        }

        impl<'map, 'elem> std::iter::ExactSizeIterator
            for $crate::map::ValueIter<'map, 'elem, $type>
        {
        }
    };
}

impl_value_iter!(ValueType::Int, i64, get_int_raw_unchecked);
impl_value_iter!(ValueType::Float, f64, get_float_raw_unchecked);
impl_value_iter!(ValueType::Data, &'map [u8], get_data_raw_unchecked);
impl_value_iter!(
    ValueType::VideoNode,
    Node<'elem>,
    get_video_node_raw_unchecked
);
impl_value_iter!(
    ValueType::VideoFrame,
    FrameRef<'elem>,
    get_video_frame_raw_unchecked
);
impl_value_iter!(
    ValueType::Function,
    Function<'elem>,
    get_function_raw_unchecked
);
