//! VapourSynth maps.

use std::borrow::Cow;
use std::ffi::{CStr, CString};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use std::{result, slice};
use vapoursynth_sys as ffi;

use crate::api::API;
use crate::frame::{Frame, FrameRef};
use crate::function::Function;
use crate::node::Node;

mod errors;
pub use self::errors::{Error, InvalidKeyError, Result};

mod iterators;
pub use self::iterators::{Keys, ValueIter};

mod value;
pub use self::value::{Value, ValueType};

/// A VapourSynth map.
///
/// A map contains key-value pairs where the value is zero or more elements of a certain type.
// This type is intended to be publicly used only in reference form.
#[derive(Debug)]
pub struct Map<'elem> {
    // The actual mutability of this depends on whether it's accessed via `&Map` or `&mut Map`.
    handle: NonNull<ffi::VSMap>,
    _elem: PhantomData<&'elem ()>,
}

/// A reference to a VapourSynth map.
#[derive(Debug)]
pub struct MapRef<'owner, 'elem> {
    // Only immutable references to this are allowed.
    map: Map<'elem>,
    _owner: PhantomData<&'owner ()>,
}

/// A reference to a mutable VapourSynth map.
#[derive(Debug)]
pub struct MapRefMut<'owner, 'elem> {
    // Both mutable and immutable references to this are allowed.
    map: Map<'elem>,
    _owner: PhantomData<&'owner ()>,
}

/// An owned VapourSynth map.
#[derive(Debug)]
pub struct OwnedMap<'elem> {
    // Both mutable and immutable references to this are allowed.
    map: Map<'elem>,
}

unsafe impl<'elem> Send for Map<'elem> {}
unsafe impl<'elem> Sync for Map<'elem> {}

#[doc(hidden)]
impl<'elem> Deref for Map<'elem> {
    type Target = ffi::VSMap;

    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { self.handle.as_ref() }
    }
}

#[doc(hidden)]
impl<'elem> DerefMut for Map<'elem> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.handle.as_mut() }
    }
}

impl<'owner, 'elem> Deref for MapRef<'owner, 'elem> {
    type Target = Map<'elem>;

    // Technically this should return `&'owner`.
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl<'owner, 'elem> Deref for MapRefMut<'owner, 'elem> {
    type Target = Map<'elem>;

    // Technically this should return `&'owner`.
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl<'owner, 'elem> DerefMut for MapRefMut<'owner, 'elem> {
    // Technically this should return `&'owner`.
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.map
    }
}

impl<'elem> Drop for OwnedMap<'elem> {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            API::get_cached().free_map(&mut self.map);
        }
    }
}

impl<'elem> Deref for OwnedMap<'elem> {
    type Target = Map<'elem>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl<'elem> DerefMut for OwnedMap<'elem> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.map
    }
}

impl<'elem> OwnedMap<'elem> {
    /// Creates a new map.
    #[inline]
    pub fn new(api: API) -> Self {
        Self {
            map: unsafe { Map::from_ptr(api.create_map()) },
        }
    }

    /// Wraps pointer into `OwnedMap`.
    ///
    /// # Safety
    /// The caller needs to ensure the pointer and the lifetime is valid and that this is an owned
    /// map pointer.
    #[inline]
    pub(crate) unsafe fn from_ptr(handle: *mut ffi::VSMap) -> Self {
        Self {
            map: unsafe { Map::from_ptr(handle) },
        }
    }
}

impl<'owner, 'elem> MapRef<'owner, 'elem> {
    /// Wraps pointer into `MapRef`.
    ///
    /// # Safety
    /// The caller needs to ensure the pointer and the lifetimes are valid, and that there are no
    /// mutable references to the given map.
    #[inline]
    pub(crate) unsafe fn from_ptr(handle: *const ffi::VSMap) -> Self {
        Self {
            map: unsafe { Map::from_ptr(handle) },
            _owner: PhantomData,
        }
    }
}

impl<'owner, 'elem> MapRefMut<'owner, 'elem> {
    /// Wraps pointer into `MapRefMut`.
    ///
    /// # Safety
    /// The caller needs to ensure the pointer and the lifetimes are valid, and that there are no
    /// references to the given map.
    #[inline]
    pub(crate) unsafe fn from_ptr(handle: *mut ffi::VSMap) -> Self {
        Self {
            map: unsafe { Map::from_ptr(handle) },
            _owner: PhantomData,
        }
    }
}

/// Turns a `prop_get_something()` error into a `Result`.
#[inline]
fn handle_get_prop_error(error: i32) -> Result<()> {
    if error == 0 {
        Ok(())
    } else {
        Err(match error {
            x if x == ffi::VSMapPropertyError_peUnset as i32 => Error::KeyNotFound,
            x if x == ffi::VSMapPropertyError_peType as i32 => Error::WrongValueType,
            x if x == ffi::VSMapPropertyError_peIndex as i32 => Error::IndexOutOfBounds,
            _ => Error::UnknownError,
        })
    }
}

/// Turns a `prop_set_something(paAppend)` error into a `Result`.
#[inline]
fn handle_append_prop_error(error: i32) -> Result<()> {
    if error != 0 {
        debug_assert!(error == 1);
        Err(Error::WrongValueType)
    } else {
        Ok(())
    }
}

impl<'elem> Map<'elem> {
    /// Wraps pointer into `Map`.
    ///
    /// # Safety
    /// The caller needs to ensure the pointer is valid, the element lifetime is valid, and that
    /// the resulting `Map` gets put into `MapRef` or `MapRefMut` or `OwnedMap` correctly.
    #[inline]
    pub(crate) unsafe fn from_ptr(handle: *const ffi::VSMap) -> Self {
        Self {
            handle: unsafe { NonNull::new_unchecked(handle as *mut ffi::VSMap) },
            _elem: PhantomData,
        }
    }

    /// Checks if the key is valid. Valid keys start with an alphabetic character or an underscore,
    /// and contain only alphanumeric characters and underscores.
    pub fn is_key_valid(key: &str) -> result::Result<(), InvalidKeyError> {
        if key.is_empty() {
            return Err(InvalidKeyError::EmptyKey);
        }

        let mut chars = key.chars();

        let first = chars.next().unwrap();
        if !first.is_ascii_alphabetic() && first != '_' {
            return Err(InvalidKeyError::InvalidCharacter(0));
        }

        for (i, c) in chars.enumerate() {
            if !c.is_ascii_alphanumeric() && c != '_' {
                return Err(InvalidKeyError::InvalidCharacter(i + 1));
            }
        }

        Ok(())
    }

    /// Checks if the key is valid and makes it a `CString`.
    #[inline]
    pub(crate) fn make_raw_key(key: &str) -> Result<CString> {
        Map::is_key_valid(key)?;
        Ok(CString::new(key).unwrap())
    }

    /// Clears the map.
    #[inline]
    pub fn clear(&mut self) {
        unsafe {
            API::get_cached().clear_map(self);
        }
    }

    /// Returns the error message contained in the map, if any.
    #[inline]
    pub fn error(&self) -> Option<Cow<'_, str>> {
        let error_message = unsafe { API::get_cached().get_error(self) };
        if error_message.is_null() {
            return None;
        }

        let error_message = unsafe { CStr::from_ptr(error_message) };
        Some(error_message.to_string_lossy())
    }

    /// Adds an error message to a map. The map is cleared first.
    #[inline]
    pub fn set_error(&mut self, error_message: &str) -> Result<()> {
        let error_message = CString::new(error_message)?;
        unsafe {
            API::get_cached().set_error(self, error_message.as_ptr());
        }
        Ok(())
    }

    /// Returns the number of keys contained in a map.
    #[inline]
    pub fn key_count(&self) -> usize {
        let count = unsafe { API::get_cached().prop_num_keys(self) };
        debug_assert!(count >= 0);
        count as usize
    }

    /// Returns a key from a map.
    ///
    /// # Panics
    /// Panics if `index >= self.key_count()`.
    #[inline]
    pub(crate) fn key_raw(&self, index: usize) -> &CStr {
        assert!(index < self.key_count());
        let index = index as i32;

        unsafe { CStr::from_ptr(API::get_cached().prop_get_key(self, index)) }
    }

    /// Returns a key from a map.
    ///
    /// # Panics
    /// Panics if `index >= self.key_count()`.
    #[inline]
    pub fn key(&self, index: usize) -> &str {
        self.key_raw(index).to_str().unwrap()
    }

    /// Returns an iterator over all keys in a map.
    #[inline]
    pub fn keys(&self) -> Keys<'_, '_> {
        Keys::new(self)
    }

    /// Returns the number of elements associated with a key in a map.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn value_count_raw_unchecked(&self, key: &CStr) -> Result<usize> {
        let rv = unsafe { API::get_cached().prop_num_elements(self, key.as_ptr()) };
        if rv == -1 {
            Err(Error::KeyNotFound)
        } else {
            debug_assert!(rv >= 0);
            Ok(rv as usize)
        }
    }

    /// Returns the number of elements associated with a key in a map.
    #[inline]
    pub fn value_count(&self, key: &str) -> Result<usize> {
        let key = Map::make_raw_key(key)?;
        unsafe { self.value_count_raw_unchecked(&key) }
    }

    /// Retrieves a value type from a map.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn value_type_raw_unchecked(&self, key: &CStr) -> Result<ValueType> {
        match unsafe { API::get_cached().prop_get_type(self, key.as_ptr()) } {
            x if x == ffi::VSPropertyType_ptUnset as i32 => Err(Error::KeyNotFound),
            x if x == ffi::VSPropertyType_ptInt as i32 => Ok(ValueType::Int),
            x if x == ffi::VSPropertyType_ptFloat as i32 => Ok(ValueType::Float),
            x if x == ffi::VSPropertyType_ptData as i32 => Ok(ValueType::Data),
            x if x == ffi::VSPropertyType_ptVideoNode as i32 => Ok(ValueType::VideoNode),
            x if x == ffi::VSPropertyType_ptAudioNode as i32 => Ok(ValueType::AudioNode),
            x if x == ffi::VSPropertyType_ptVideoFrame as i32 => Ok(ValueType::VideoFrame),
            x if x == ffi::VSPropertyType_ptAudioFrame as i32 => Ok(ValueType::AudioFrame),
            x if x == ffi::VSPropertyType_ptFunction as i32 => Ok(ValueType::Function),
            _ => unreachable!(),
        }
    }

    /// Retrieves a value type from a map.
    #[inline]
    pub fn value_type(&self, key: &str) -> Result<ValueType> {
        let key = Map::make_raw_key(key)?;
        unsafe { self.value_type_raw_unchecked(&key) }
    }

    /// Deletes the given key.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn delete_key_raw_unchecked(&mut self, key: &CStr) -> Result<()> {
        let result = unsafe { API::get_cached().prop_delete_key(self, key.as_ptr()) };
        if result == 0 {
            Err(Error::KeyNotFound)
        } else {
            debug_assert!(result == 1);
            Ok(())
        }
    }

    /// Deletes the given key.
    #[inline]
    pub fn delete_key(&mut self, key: &str) -> Result<()> {
        let key = Map::make_raw_key(key)?;
        unsafe { self.delete_key_raw_unchecked(&key) }
    }

    /// Retrieves a property value.
    #[inline]
    pub fn get<'map, T: Value<'map, 'elem>>(&'map self, key: &str) -> Result<T> {
        T::get_from_map(self, key)
    }

    /// Retrieves an iterator over the map values.
    #[inline]
    pub fn get_iter<'map, T: Value<'map, 'elem>>(
        &'map self,
        key: &str,
    ) -> Result<ValueIter<'map, 'elem, T>> {
        T::get_iter_from_map(self, key)
    }

    /// Sets a property value.
    #[inline]
    pub fn set<'map, T: Value<'map, 'elem>>(&'map mut self, key: &str, x: &T) -> Result<()> {
        T::store_in_map(self, key, x)
    }

    /// Appends a property value.
    #[inline]
    pub fn append<'map, T: Value<'map, 'elem>>(&'map mut self, key: &str, x: &T) -> Result<()> {
        T::append_to_map(self, key, x)
    }

    /// Retrieves an integer from a map.
    ///
    /// This function retrieves the first value associated with the key.
    #[inline]
    pub fn get_int(&self, key: &str) -> Result<i64> {
        let key = Map::make_raw_key(key)?;
        unsafe { self.get_int_raw_unchecked(&key, 0) }
    }

    /// Retrieves integers from a map.
    #[inline]
    pub fn get_int_iter<'map>(&'map self, key: &str) -> Result<ValueIter<'map, 'elem, i64>> {
        let key = Map::make_raw_key(key)?;
        unsafe { ValueIter::<i64>::new(self, key) }
    }

    /// Retrieves an array of integers from a map.
    ///
    /// This is faster than iterating over a `get_int_iter()`.
    #[inline]
    pub fn get_int_array(&self, key: &str) -> Result<&[i64]> {
        let key = Map::make_raw_key(key)?;
        unsafe { self.get_int_array_raw_unchecked(&key) }
    }

    /// Retrieves a floating point number from a map.
    ///
    /// This function retrieves the first value associated with the key.
    #[inline]
    pub fn get_float(&self, key: &str) -> Result<f64> {
        let key = Map::make_raw_key(key)?;
        unsafe { self.get_float_raw_unchecked(&key, 0) }
    }

    /// Retrieves an array of floating point numbers from a map.
    ///
    /// This is faster than iterating over a `get_float_iter()`.
    #[inline]
    pub fn get_float_array(&self, key: &str) -> Result<&[f64]> {
        let key = Map::make_raw_key(key)?;
        unsafe { self.get_float_array_raw_unchecked(&key) }
    }

    /// Retrieves floating point numbers from a map.
    #[inline]
    pub fn get_float_iter<'map>(&'map self, key: &str) -> Result<ValueIter<'map, 'elem, f64>> {
        let key = Map::make_raw_key(key)?;
        unsafe { ValueIter::<f64>::new(self, key) }
    }

    /// Retrieves data from a map.
    ///
    /// This function retrieves the first value associated with the key.
    #[inline]
    pub fn get_data(&self, key: &str) -> Result<&[u8]> {
        let key = Map::make_raw_key(key)?;
        unsafe { self.get_data_raw_unchecked(&key, 0) }
    }

    /// Retrieves data from a map.
    #[inline]
    pub fn get_data_iter<'map>(
        &'map self,
        key: &str,
    ) -> Result<ValueIter<'map, 'elem, &'map [u8]>> {
        let key = Map::make_raw_key(key)?;
        unsafe { ValueIter::<&[u8]>::new(self, key) }
    }

    /// Retrieves a node from a map.
    ///
    /// This function retrieves the first value associated with the key.
    #[inline]
    pub fn get_video_node(&self, key: &str) -> Result<Node<'elem>> {
        let key = Map::make_raw_key(key)?;
        unsafe { self.get_video_node_raw_unchecked(&key, 0) }
    }

    /// Retrieves nodes from a map.
    #[inline]
    pub fn get_video_node_iter<'map>(
        &'map self,
        key: &str,
    ) -> Result<ValueIter<'map, 'elem, Node<'elem>>> {
        let key = Map::make_raw_key(key)?;
        unsafe { ValueIter::<Node>::new(self, key) }
    }

    /// Retrieves a frame from a map.
    ///
    /// This function retrieves the first value associated with the key.
    #[inline]
    pub fn get_video_frame(&self, key: &str) -> Result<FrameRef<'elem>> {
        let key = Map::make_raw_key(key)?;
        unsafe { self.get_video_frame_raw_unchecked(&key, 0) }
    }

    /// Retrieves frames from a map.
    #[inline]
    pub fn get_video_frame_iter<'map>(
        &'map self,
        key: &str,
    ) -> Result<ValueIter<'map, 'elem, FrameRef<'elem>>> {
        let key = Map::make_raw_key(key)?;
        unsafe { ValueIter::<FrameRef>::new(self, key) }
    }

    /// Retrieves a function from a map.
    ///
    /// This function retrieves the first value associated with the key.
    #[inline]
    pub fn get_function(&self, key: &str) -> Result<Function<'elem>> {
        let key = Map::make_raw_key(key)?;
        unsafe { self.get_function_raw_unchecked(&key, 0) }
    }

    /// Retrieves functions from a map.
    #[inline]
    pub fn get_function_iter<'map>(
        &'map self,
        key: &str,
    ) -> Result<ValueIter<'map, 'elem, Function<'elem>>> {
        let key = Map::make_raw_key(key)?;
        unsafe { ValueIter::<Function>::new(self, key) }
    }

    /// Retrieves an integer from a map.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn get_int_raw_unchecked(&self, key: &CStr, index: i32) -> Result<i64> {
        let mut error = 0;
        let value =
            unsafe { API::get_cached().prop_get_int(self, key.as_ptr(), index, &mut error) };
        handle_get_prop_error(error)?;

        Ok(value)
    }

    /// Retrieves an array of integers from a map.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn get_int_array_raw_unchecked(&self, key: &CStr) -> Result<&[i64]> {
        let mut error = 0;
        let value = unsafe { API::get_cached().prop_get_int_array(self, key.as_ptr(), &mut error) };
        handle_get_prop_error(error)?;

        unsafe {
            let length = self.value_count_raw_unchecked(key).unwrap();
            Ok(slice::from_raw_parts(value, length))
        }
    }

    /// Retrieves a floating point number from a map.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn get_float_raw_unchecked(&self, key: &CStr, index: i32) -> Result<f64> {
        let mut error = 0;
        let value =
            unsafe { API::get_cached().prop_get_float(self, key.as_ptr(), index, &mut error) };
        handle_get_prop_error(error)?;

        Ok(value)
    }

    /// Retrieves an array of floating point numbers from a map.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn get_float_array_raw_unchecked(&self, key: &CStr) -> Result<&[f64]> {
        let mut error = 0;
        let value =
            unsafe { API::get_cached().prop_get_float_array(self, key.as_ptr(), &mut error) };
        handle_get_prop_error(error)?;

        unsafe {
            let length = self.value_count_raw_unchecked(key).unwrap();
            Ok(slice::from_raw_parts(value, length))
        }
    }

    /// Retrieves data from a map.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn get_data_raw_unchecked(&self, key: &CStr, index: i32) -> Result<&[u8]> {
        let mut error = 0;
        let value =
            unsafe { API::get_cached().prop_get_data(self, key.as_ptr(), index, &mut error) };
        handle_get_prop_error(error)?;

        let mut error = 0;
        let length =
            unsafe { API::get_cached().prop_get_data_size(self, key.as_ptr(), index, &mut error) };
        debug_assert!(error == 0);
        debug_assert!(length >= 0);

        unsafe { Ok(slice::from_raw_parts(value as *const u8, length as usize)) }
    }

    /// Retrieves a node from a map.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn get_video_node_raw_unchecked(
        &self,
        key: &CStr,
        index: i32,
    ) -> Result<Node<'elem>> {
        let mut error = 0;
        let value =
            unsafe { API::get_cached().prop_get_node(self, key.as_ptr(), index, &mut error) };
        handle_get_prop_error(error)?;

        unsafe { Ok(Node::from_ptr(value)) }
    }

    /// Retrieves a frame from a map.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn get_video_frame_raw_unchecked(
        &self,
        key: &CStr,
        index: i32,
    ) -> Result<FrameRef<'elem>> {
        let mut error = 0;
        let value =
            unsafe { API::get_cached().prop_get_frame(self, key.as_ptr(), index, &mut error) };
        handle_get_prop_error(error)?;

        unsafe { Ok(FrameRef::from_ptr(value)) }
    }

    /// Retrieves a function from a map.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn get_function_raw_unchecked(
        &self,
        key: &CStr,
        index: i32,
    ) -> Result<Function<'elem>> {
        let mut error = 0;
        let value =
            unsafe { API::get_cached().prop_get_func(self, key.as_ptr(), index, &mut error) };
        handle_get_prop_error(error)?;

        unsafe { Ok(Function::from_ptr(value)) }
    }

    /// Appends an integer to a map.
    #[inline]
    pub fn append_int(&mut self, key: &str, x: i64) -> Result<()> {
        let key = Map::make_raw_key(key)?;
        unsafe { self.append_int_raw_unchecked(&key, x) }
    }

    /// Appends a floating point number to a map.
    #[inline]
    pub fn append_float(&mut self, key: &str, x: f64) -> Result<()> {
        let key = Map::make_raw_key(key)?;
        unsafe { self.append_float_raw_unchecked(&key, x) }
    }

    /// Appends data to a map.
    #[inline]
    pub fn append_data(&mut self, key: &str, x: &[u8]) -> Result<()> {
        let key = Map::make_raw_key(key)?;
        unsafe { self.append_data_raw_unchecked(&key, x) }
    }

    /// Appends a node to a map.
    #[inline]
    pub fn append_node(&mut self, key: &str, x: &Node<'elem>) -> Result<()> {
        let key = Map::make_raw_key(key)?;
        unsafe { self.append_node_raw_unchecked(&key, x) }
    }

    /// Appends a frame to a map.
    #[inline]
    pub fn append_frame(&mut self, key: &str, x: &Frame<'elem>) -> Result<()> {
        let key = Map::make_raw_key(key)?;
        unsafe { self.append_frame_raw_unchecked(&key, x) }
    }

    /// Appends a function to a map.
    #[inline]
    pub fn append_function(&mut self, key: &str, x: &Function<'elem>) -> Result<()> {
        let key = Map::make_raw_key(key)?;
        unsafe { self.append_function_raw_unchecked(&key, x) }
    }

    /// Appends an integer to a map.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn append_int_raw_unchecked(&mut self, key: &CStr, x: i64) -> Result<()> {
        let error = unsafe {
            API::get_cached().prop_set_int(self, key.as_ptr(), x, ffi::VSMapAppendMode_maAppend)
        };

        handle_append_prop_error(error)
    }

    /// Appends a floating point number to a map.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn append_float_raw_unchecked(&mut self, key: &CStr, x: f64) -> Result<()> {
        let error = unsafe {
            API::get_cached().prop_set_float(self, key.as_ptr(), x, ffi::VSMapAppendMode_maAppend)
        };

        handle_append_prop_error(error)
    }

    /// Appends data to a map.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn append_data_raw_unchecked(&mut self, key: &CStr, x: &[u8]) -> Result<()> {
        let error = unsafe {
            API::get_cached().prop_set_data(self, key.as_ptr(), x, ffi::VSMapAppendMode_maAppend)
        };

        handle_append_prop_error(error)
    }

    /// Appends a node to a map.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn append_node_raw_unchecked(
        &mut self,
        key: &CStr,
        x: &Node<'elem>,
    ) -> Result<()> {
        let error = unsafe {
            API::get_cached().prop_set_node(
                self,
                key.as_ptr(),
                x.ptr(),
                ffi::VSMapAppendMode_maAppend,
            )
        };

        handle_append_prop_error(error)
    }

    /// Appends a frame to a map.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn append_frame_raw_unchecked(
        &mut self,
        key: &CStr,
        x: &Frame<'elem>,
    ) -> Result<()> {
        let error = unsafe {
            API::get_cached().prop_set_frame(
                self,
                key.as_ptr(),
                x.deref(),
                ffi::VSMapAppendMode_maAppend,
            )
        };

        handle_append_prop_error(error)
    }

    /// Appends a function to a map.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn append_function_raw_unchecked(
        &mut self,
        key: &CStr,
        x: &Function<'elem>,
    ) -> Result<()> {
        let error = unsafe {
            API::get_cached().prop_set_func(
                self,
                key.as_ptr(),
                x.ptr(),
                ffi::VSMapAppendMode_maAppend,
            )
        };

        handle_append_prop_error(error)
    }

    /// Sets a property value to an integer.
    #[inline]
    pub fn set_int(&mut self, key: &str, x: i64) -> Result<()> {
        let key = Map::make_raw_key(key)?;
        unsafe {
            self.set_int_raw_unchecked(&key, x);
        }
        Ok(())
    }

    /// Sets a property value to an integer array.
    ///
    /// This is faster than calling `append_int()` in a loop.
    #[inline]
    pub fn set_int_array(&mut self, key: &str, x: &[i64]) -> Result<()> {
        let key = Map::make_raw_key(key)?;
        unsafe {
            self.set_int_array_raw_unchecked(&key, x);
        }
        Ok(())
    }

    /// Sets a property value to a floating point number.
    #[inline]
    pub fn set_float(&mut self, key: &str, x: f64) -> Result<()> {
        let key = Map::make_raw_key(key)?;
        unsafe {
            self.set_float_raw_unchecked(&key, x);
        }
        Ok(())
    }

    /// Sets a property value to a floating point number array.
    ///
    /// This is faster than calling `append_float()` in a loop.
    #[inline]
    pub fn set_float_array(&mut self, key: &str, x: &[f64]) -> Result<()> {
        let key = Map::make_raw_key(key)?;
        unsafe {
            self.set_float_array_raw_unchecked(&key, x);
        }
        Ok(())
    }

    /// Sets a property value to data.
    #[inline]
    pub fn set_data(&mut self, key: &str, x: &[u8]) -> Result<()> {
        let key = Map::make_raw_key(key)?;
        unsafe {
            self.set_data_raw_unchecked(&key, x);
        }
        Ok(())
    }

    /// Sets a property value to a node.
    #[inline]
    pub fn set_node(&mut self, key: &str, x: &Node<'elem>) -> Result<()> {
        let key = Map::make_raw_key(key)?;
        unsafe {
            self.set_node_raw_unchecked(&key, x);
        }
        Ok(())
    }

    /// Sets a property value to a frame.
    #[inline]
    pub fn set_frame(&mut self, key: &str, x: &Frame<'elem>) -> Result<()> {
        let key = Map::make_raw_key(key)?;
        unsafe {
            self.set_frame_raw_unchecked(&key, x);
        }
        Ok(())
    }

    /// Sets a property value to a function.
    #[inline]
    pub fn set_function(&mut self, key: &str, x: &Function<'elem>) -> Result<()> {
        let key = Map::make_raw_key(key)?;
        unsafe {
            self.set_function_raw_unchecked(&key, x);
        }
        Ok(())
    }

    /// Sets a property value to an integer.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn set_int_raw_unchecked(&mut self, key: &CStr, x: i64) {
        let error = unsafe {
            API::get_cached().prop_set_int(self, key.as_ptr(), x, ffi::VSMapAppendMode_maReplace)
        };

        debug_assert!(error == 0);
    }

    /// Sets a property value to an integer array.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    ///
    /// # Panics
    /// Panics if `x.len()` can't fit in an `i32`.
    #[inline]
    pub(crate) unsafe fn set_int_array_raw_unchecked(&mut self, key: &CStr, x: &[i64]) {
        let error = unsafe { API::get_cached().prop_set_int_array(self, key.as_ptr(), x) };

        debug_assert!(error == 0);
    }

    /// Sets a property value to a floating point number.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn set_float_raw_unchecked(&mut self, key: &CStr, x: f64) {
        let error = unsafe {
            API::get_cached().prop_set_float(self, key.as_ptr(), x, ffi::VSMapAppendMode_maReplace)
        };

        debug_assert!(error == 0);
    }

    /// Sets a property value to a floating point number array.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    ///
    /// # Panics
    /// Panics if `x.len()` can't fit in an `i32`.
    #[inline]
    pub(crate) unsafe fn set_float_array_raw_unchecked(&mut self, key: &CStr, x: &[f64]) {
        let error = unsafe { API::get_cached().prop_set_float_array(self, key.as_ptr(), x) };

        debug_assert!(error == 0);
    }

    /// Sets a property value to data.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn set_data_raw_unchecked(&mut self, key: &CStr, x: &[u8]) {
        let error = unsafe {
            API::get_cached().prop_set_data(self, key.as_ptr(), x, ffi::VSMapAppendMode_maReplace)
        };

        debug_assert!(error == 0);
    }

    /// Sets a property value to a node.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn set_node_raw_unchecked(&mut self, key: &CStr, x: &Node<'elem>) {
        let error = unsafe {
            API::get_cached().prop_set_node(
                self,
                key.as_ptr(),
                x.ptr(),
                ffi::VSMapAppendMode_maReplace,
            )
        };

        debug_assert!(error == 0);
    }

    /// Sets a property value to a frame.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn set_frame_raw_unchecked(&mut self, key: &CStr, x: &Frame<'elem>) {
        let error = unsafe {
            API::get_cached().prop_set_frame(
                self,
                key.as_ptr(),
                x.deref(),
                ffi::VSMapAppendMode_maReplace,
            )
        };

        debug_assert!(error == 0);
    }

    /// Sets a property value to a function.
    ///
    /// # Safety
    /// The caller must ensure `key` is valid.
    #[inline]
    pub(crate) unsafe fn set_function_raw_unchecked(&mut self, key: &CStr, x: &Function<'elem>) {
        let error = unsafe {
            API::get_cached().prop_set_func(
                self,
                key.as_ptr(),
                x.ptr(),
                ffi::VSMapAppendMode_maReplace,
            )
        };

        debug_assert!(error == 0);
    }
}
