use crate::frame::FrameRef;
use crate::function::Function;
use crate::map::{Map, Result, ValueIter};
use crate::node::Node;

/// An enumeration of all possible value types.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum ValueType {
    Int,
    Float,
    Data,
    VideoNode,
    AudioNode,
    VideoFrame,
    AudioFrame,
    Function,
}

/// A trait for values which can be stored in a map.
pub trait Value<'map, 'elem: 'map>: Sized {
    /// Retrieves the value from the map.
    fn get_from_map(map: &'map Map<'elem>, key: &str) -> Result<Self>;

    /// Retrieves an iterator over the values from the map.
    fn get_iter_from_map(map: &'map Map<'elem>, key: &str) -> Result<ValueIter<'map, 'elem, Self>>;

    /// Sets the property value in the map.
    fn store_in_map(map: &'map mut Map<'elem>, key: &str, x: &Self) -> Result<()>;

    /// Appends the value to the map.
    fn append_to_map(map: &'map mut Map<'elem>, key: &str, x: &Self) -> Result<()>;
}

impl<'map, 'elem: 'map> Value<'map, 'elem> for i64 {
    #[inline]
    fn get_from_map(map: &Map, key: &str) -> Result<Self> {
        map.get_int(key)
    }

    #[inline]
    fn get_iter_from_map(map: &'map Map<'elem>, key: &str) -> Result<ValueIter<'map, 'elem, Self>> {
        map.get_int_iter(key)
    }

    #[inline]
    fn store_in_map(map: &mut Map, key: &str, x: &Self) -> Result<()> {
        map.set_int(key, *x)
    }

    #[inline]
    fn append_to_map(map: &mut Map, key: &str, x: &Self) -> Result<()> {
        map.append_int(key, *x)
    }
}

impl<'map, 'elem: 'map> Value<'map, 'elem> for f64 {
    fn get_from_map(map: &Map, key: &str) -> Result<Self> {
        map.get_float(key)
    }

    #[inline]
    fn get_iter_from_map(map: &'map Map<'elem>, key: &str) -> Result<ValueIter<'map, 'elem, Self>> {
        map.get_float_iter(key)
    }

    #[inline]
    fn store_in_map(map: &mut Map, key: &str, x: &Self) -> Result<()> {
        map.set_float(key, *x)
    }

    #[inline]
    fn append_to_map(map: &mut Map, key: &str, x: &Self) -> Result<()> {
        map.append_float(key, *x)
    }
}

impl<'map, 'elem: 'map> Value<'map, 'elem> for &'map [u8] {
    #[inline]
    fn get_from_map(map: &'map Map, key: &str) -> Result<Self> {
        map.get_data(key)
    }

    #[inline]
    fn get_iter_from_map(map: &'map Map<'elem>, key: &str) -> Result<ValueIter<'map, 'elem, Self>> {
        map.get_data_iter(key)
    }

    #[inline]
    fn store_in_map(map: &'map mut Map, key: &str, x: &Self) -> Result<()> {
        map.set_data(key, x)
    }

    #[inline]
    fn append_to_map(map: &'map mut Map, key: &str, x: &Self) -> Result<()> {
        map.append_data(key, x)
    }
}

impl<'map, 'elem: 'map> Value<'map, 'elem> for Node<'elem> {
    #[inline]
    fn get_from_map(map: &Map<'elem>, key: &str) -> Result<Self> {
        map.get_video_node(key)
    }

    #[inline]
    fn get_iter_from_map(map: &'map Map<'elem>, key: &str) -> Result<ValueIter<'map, 'elem, Self>> {
        map.get_video_node_iter(key)
    }

    #[inline]
    fn store_in_map(map: &mut Map<'elem>, key: &str, x: &Self) -> Result<()> {
        map.set_node(key, x)
    }

    #[inline]
    fn append_to_map(map: &mut Map<'elem>, key: &str, x: &Self) -> Result<()> {
        map.append_node(key, x)
    }
}

impl<'map, 'elem: 'map> Value<'map, 'elem> for FrameRef<'elem> {
    #[inline]
    fn get_from_map(map: &Map<'elem>, key: &str) -> Result<Self> {
        map.get_video_frame(key)
    }

    #[inline]
    fn get_iter_from_map(map: &'map Map<'elem>, key: &str) -> Result<ValueIter<'map, 'elem, Self>> {
        map.get_video_frame_iter(key)
    }

    #[inline]
    fn store_in_map(map: &mut Map<'elem>, key: &str, x: &Self) -> Result<()> {
        map.set_frame(key, x)
    }

    #[inline]
    fn append_to_map(map: &mut Map<'elem>, key: &str, x: &Self) -> Result<()> {
        map.append_frame(key, x)
    }
}

impl<'map, 'elem: 'map> Value<'map, 'elem> for Function<'elem> {
    #[inline]
    fn get_from_map(map: &Map<'elem>, key: &str) -> Result<Self> {
        map.get_function(key)
    }

    #[inline]
    fn get_iter_from_map(map: &'map Map<'elem>, key: &str) -> Result<ValueIter<'map, 'elem, Self>> {
        map.get_function_iter(key)
    }

    #[inline]
    fn store_in_map(map: &mut Map<'elem>, key: &str, x: &Self) -> Result<()> {
        map.set_function(key, x)
    }

    #[inline]
    fn append_to_map(map: &mut Map<'elem>, key: &str, x: &Self) -> Result<()> {
        map.append_function(key, x)
    }
}
