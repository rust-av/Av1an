//! Things related to making VapourSynth plugins.

use anyhow::Result;

use crate::api::API;
use crate::core::CoreRef;
use crate::frame::FrameRef;
use crate::function::Function;
use crate::map::{self, Map, Value, ValueIter};
use crate::node::Node;
use crate::video_info::VideoInfo;

mod frame_context;
pub use self::frame_context::FrameContext;

pub mod ffi;

/// Plugin metadata.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct Metadata {
    /// A "reverse" URL, unique among all plugins.
    ///
    /// For example, `com.example.invert`.
    pub identifier: &'static str,

    /// Namespace where the plugin's filters will go, unique among all plugins.
    ///
    /// Only lowercase letters and the underscore should be used, and it shouldn't be too long.
    /// Additionally, words that are special to Python, e.g. `del`, should be avoided.
    ///
    /// For example, `invert`.
    pub namespace: &'static str,

    /// Plugin name in readable form.
    ///
    /// For example, `Invert Example Plugin`.
    pub name: &'static str,

    /// Whether new filters can be registered at runtime.
    ///
    /// This should generally be set to `false`. It's used for the built-in AviSynth compat plugin.
    pub read_only: bool,
}

/// A filter function interface.
///
/// See the `make_filter_function!` macro that generates types implementing this automatically.
pub trait FilterFunction: Send + Sync {
    /// Returns the name of the function.
    ///
    /// The characters allowed are letters, numbers, and the underscore. The first character must
    /// be a letter. In other words: `^[a-zA-Z][a-zA-Z0-9_]*$`.
    ///
    /// For example, `Invert`.
    fn name(&self) -> &str;

    /// Returns the argument string.
    ///
    /// Arguments are separated by a semicolon. Each argument is made of several fields separated
    /// by a colon. Don’t insert additional whitespace characters, or VapourSynth will die.
    ///
    /// Fields:
    /// - The argument name. The same characters are allowed as for the filter's name. Argument
    ///   names should be all lowercase and use only letters and the underscore.
    ///
    /// - The type. One of `int`, `float`, `data`, `clip`, `frame`, `func`. They correspond to the
    ///   `Map::get_*()` functions (`clip` is `get_node()`). It's possible to declare an array by
    ///   appending `[]` to the type.
    ///
    /// - `opt` if the parameter is optional.
    ///
    /// - `empty` if the array is allowed to be empty.
    ///
    /// The following example declares the arguments "blah", "moo", and "asdf":
    /// `blah:clip;moo:int[]:opt;asdf:float:opt;`
    fn args(&self) -> &str;

    /// The callback for this filter function.
    ///
    /// In most cases this is where you should create a new instance of the filter and return it.
    /// However, a filter function like AviSynth compat's `LoadPlugin()` which isn't actually a
    /// filter, can return `None`.
    ///
    /// `args` contains the filter arguments, as specified by the argument string from
    /// `FilterFunction::args()`. Their presence and types are validated by VapourSynth so it's
    /// safe to `unwrap()`.
    ///
    /// In this function you should take all input nodes for your filter and store them somewhere
    /// so that you can request their frames in `get_frame_initial()`.
    // TODO: with generic associated types it'll be possible to make Filter<'core> an associated
    // type of this trait and get rid of this Box.
    fn create<'core>(
        &self,
        api: API,
        core: CoreRef<'core>,
        args: &Map<'core>,
    ) -> Result<Option<Box<dyn Filter<'core> + 'core>>>;
}

/// A filter interface.
// TODO: perhaps it's possible to figure something out about Send + Sync with specialization? Since
// there are Node flags which say that the filter will be called strictly by one thread, in which
// case Sync shouldn't be required.
pub trait Filter<'core>: Send + Sync {
    /// Returns the parameters of this filter's output node.
    ///
    /// The returned vector should contain one entry for each node output index.
    fn video_info(&self, api: API, core: CoreRef<'core>) -> Vec<VideoInfo<'core>>;

    /// Requests the necessary frames from downstream nodes.
    ///
    /// This is always the first function to get called for a given frame `n`.
    ///
    /// In this function you should call `request_frame_filter()` on any input nodes that you need
    /// and return `None`. If you do not need any input frames, you should generate the output
    /// frame and return it here.
    ///
    /// Do not call `Node::get_frame()` from within this function.
    fn get_frame_initial(
        &self,
        api: API,
        core: CoreRef<'core>,
        context: FrameContext,
        n: usize,
    ) -> Result<Option<FrameRef<'core>>>;

    /// Returns the requested frame.
    ///
    /// This is always the second function to get called for a given frame `n`. If the frame was
    /// retrned from `get_frame_initial()`, this function is not called.
    ///
    /// In this function you should call `get_frame_filter()` on the input nodes to retrieve the
    /// frames you requested in `get_frame_initial()`.
    ///
    /// Do not call `Node::get_frame()` from within this function.
    fn get_frame(
        &self,
        api: API,
        core: CoreRef<'core>,
        context: FrameContext,
        n: usize,
    ) -> Result<FrameRef<'core>>;
}

/// An internal trait representing a filter argument type.
pub trait FilterArgument<'map, 'elem: 'map>: Value<'map, 'elem> + private::Sealed {
    /// Returns the VapourSynth type name for this argument type.
    fn type_name() -> &'static str;
}

/// An internal trait representing a filter parameter type (argument type + whether it's an array
/// or optional).
pub trait FilterParameter<'map, 'elem: 'map>: private::Sealed {
    /// The underlying argument type for this parameter type.
    type Argument: FilterArgument<'map, 'elem>;

    /// Returns whether this parameter is an array.
    fn is_array() -> bool;

    /// Returns whether this parameter is optional.
    fn is_optional() -> bool;

    /// Retrieves this parameter from the given map.
    fn get_from_map(map: &'map Map<'elem>, key: &str) -> Self;
}

impl<'map, 'elem: 'map> FilterArgument<'map, 'elem> for i64 {
    #[inline]
    fn type_name() -> &'static str {
        "int"
    }
}

impl<'map, 'elem: 'map> FilterArgument<'map, 'elem> for f64 {
    #[inline]
    fn type_name() -> &'static str {
        "float"
    }
}

impl<'map, 'elem: 'map> FilterArgument<'map, 'elem> for &'map [u8] {
    #[inline]
    fn type_name() -> &'static str {
        "data"
    }
}

impl<'map, 'elem: 'map> FilterArgument<'map, 'elem> for Node<'elem> {
    #[inline]
    fn type_name() -> &'static str {
        "vnode"
    }
}

impl<'map, 'elem: 'map> FilterArgument<'map, 'elem> for FrameRef<'elem> {
    #[inline]
    fn type_name() -> &'static str {
        "vframe"
    }
}

impl<'map, 'elem: 'map> FilterArgument<'map, 'elem> for Function<'elem> {
    #[inline]
    fn type_name() -> &'static str {
        "func"
    }
}

impl<'map, 'elem: 'map, T> FilterParameter<'map, 'elem> for T
where
    T: FilterArgument<'map, 'elem>,
{
    type Argument = Self;

    #[inline]
    fn is_array() -> bool {
        false
    }

    #[inline]
    fn is_optional() -> bool {
        false
    }

    #[inline]
    fn get_from_map(map: &'map Map<'elem>, key: &str) -> Self {
        Self::get_from_map(map, key).unwrap()
    }
}

impl<'map, 'elem: 'map, T> FilterParameter<'map, 'elem> for Option<T>
where
    T: FilterArgument<'map, 'elem>,
{
    type Argument = T;

    #[inline]
    fn is_array() -> bool {
        false
    }

    #[inline]
    fn is_optional() -> bool {
        true
    }

    #[inline]
    fn get_from_map(map: &'map Map<'elem>, key: &str) -> Self {
        match <Self::Argument as Value>::get_from_map(map, key) {
            Ok(x) => Some(x),
            Err(map::Error::KeyNotFound) => None,
            _ => unreachable!(),
        }
    }
}

impl<'map, 'elem: 'map, T> FilterParameter<'map, 'elem> for ValueIter<'map, 'elem, T>
where
    T: FilterArgument<'map, 'elem>,
{
    type Argument = T;

    #[inline]
    fn is_array() -> bool {
        true
    }

    #[inline]
    fn is_optional() -> bool {
        false
    }

    #[inline]
    fn get_from_map(map: &'map Map<'elem>, key: &str) -> Self {
        <Self::Argument>::get_iter_from_map(map, key).unwrap()
    }
}

impl<'map, 'elem: 'map, T> FilterParameter<'map, 'elem> for Option<ValueIter<'map, 'elem, T>>
where
    T: FilterArgument<'map, 'elem>,
{
    type Argument = T;

    #[inline]
    fn is_array() -> bool {
        true
    }

    #[inline]
    fn is_optional() -> bool {
        true
    }

    #[inline]
    fn get_from_map(map: &'map Map<'elem>, key: &str) -> Self {
        match <Self::Argument as Value>::get_iter_from_map(map, key) {
            Ok(x) => Some(x),
            Err(map::Error::KeyNotFound) => None,
            _ => unreachable!(),
        }
    }
}

mod private {
    use super::{FilterArgument, FrameRef, Function, Node, ValueIter};

    pub trait Sealed {}

    impl Sealed for i64 {}
    impl Sealed for f64 {}
    impl Sealed for &[u8] {}
    impl<'elem> Sealed for Node<'elem> {}
    impl<'elem> Sealed for FrameRef<'elem> {}
    impl<'elem> Sealed for Function<'elem> {}

    impl<'map, 'elem: 'map, T> Sealed for Option<T> where T: FilterArgument<'map, 'elem> {}

    impl<'map, 'elem: 'map, T> Sealed for ValueIter<'map, 'elem, T> where T: FilterArgument<'map, 'elem> {}

    impl<'map, 'elem: 'map, T> Sealed for Option<ValueIter<'map, 'elem, T>> where
        T: FilterArgument<'map, 'elem>
    {
    }
}

/// Make a filter function easily and avoid boilerplate.
///
/// This macro accepts the name of the filter function type, the name of the filter and the create
/// function.
///
/// The macro generates a type implementing `FilterFunction` with the correct `args()` string
/// derived from the function parameters of the specified create function. The generated
/// `FilterFunction::create()` extracts all parameters from the argument map received from
/// VapourSynth and passes them into the specified create function.
///
/// The create function should look like:
///
/// ```ignore
/// fn create<'core>(
///     api: API,
///     core: CoreRef<'core>,
///     /* filter arguments */
/// ) -> Result<Option<Box<Filter<'core> + 'core>>, Error> {
///     /* ... */
/// }
/// ```
///
/// All VapourSynth-supported types can be used, as well as `Option<T>` for optional parameters and
/// `ValueIter<T>` for array parameters. Array parameters can be empty.
///
/// Caveat: the macro doesn't currently allow specifying mutable parameters, so to do that they
/// have to be reassigned to a mutable variable in the function body. This is mainly a problem for
/// array parameters. See how the example below handles it.
///
/// Another caveat: underscore lifetimes are required for receiving `ValueIter<T>`.
///
/// # Example
/// ```ignore
/// make_filter_function! {
///     MyFilterFunction, "MyFilter"
///
///     fn create_my_filter<'core>(
///         _api: API,
///         _core: CoreRef<'core>,
///         int_parameter: i64,
///         some_data: &[u8],
///         optional_parameter: Option<f64>,
///         array_parameter: ValueIter<'_, 'core, Node<'core>>,
///         optional_array_parameter: Option<ValueIter<'_, 'core, FrameRef<'core>>>,
///     ) -> Result<Option<Box<Filter<'core> + 'core>>, Error> {
///         let mut array_parameter = array_parameter;
///         Ok(Some(Box::new(MyFilter::new(/* ... */))));
///     }
/// }
/// ```
#[macro_export]
macro_rules! make_filter_function {
    (
        $struct_name:ident, $function_name:tt

        $(#[$attr:meta])*
        fn $create_fn_name:ident<$lifetime:tt>(
            $api_arg_name:ident : $api_arg_type:ty,
            $core_arg_name:ident : $core_arg_type:ty,
            $($arg_name:ident : $arg_type:ty),* $(,)*
        ) -> $return_type:ty {
            $($body:tt)*
        }
    ) => (
        struct $struct_name {
            args: String,
        }

        impl $struct_name {
            fn new<'core>() -> Self {
                let mut args = String::new();

                $(
                    // Don't use format!() for better constant propagation.
                    args += stringify!($arg_name); // TODO: allow using a different name.
                    args += ":";
                    args
                        += <<$arg_type as $crate::plugins::FilterParameter>::Argument>::type_name();

                    if <$arg_type as $crate::plugins::FilterParameter>::is_array() {
                        args += "[]";
                    }

                    if <$arg_type as $crate::plugins::FilterParameter>::is_optional() {
                        args += ":opt";
                    }

                    args += ";";
                )*

                Self { args }
            }
        }

        impl $crate::plugins::FilterFunction for $struct_name {
            #[inline]
            fn name(&self) -> &str {
                $function_name
            }

            #[inline]
            fn args(&self) -> &str {
                &self.args
            }

            #[inline]
            fn create<'core>(
                &self,
                api: $crate::prelude::API,
                core: $crate::core::CoreRef<'core>,
                args: &$crate::prelude::Map<'core>,
            ) -> $crate::anyhow::Result<Option<Box<dyn $crate::plugins::Filter<'core> + 'core>>> {
                $create_fn_name(
                    api,
                    core,
                    $(
                        <$arg_type as $crate::plugins::FilterParameter>::get_from_map(
                            args,
                            stringify!($arg_name),
                        )
                    ),*
                )
            }
        }

        $(#[$attr])*
        fn $create_fn_name<$lifetime>(
            $api_arg_name : $api_arg_type,
            $core_arg_name : $core_arg_type,
            $($arg_name : $arg_type),*
        ) -> $return_type {
            $($body)*
        }
    )
}
