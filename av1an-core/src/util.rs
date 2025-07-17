#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

/// Count the number of elements passed to this macro.
///
/// Extra commas in between other commas are counted as an element.
#[macro_export]
macro_rules! count {
  () => (0_usize);
  ($x:tt, $($xs:tt),*) => (1_usize + $crate::count!($($xs)*));
  ($x:tt, $($xs:tt)*) => (1_usize + $crate::count!($($xs)*));
  ($x:tt $($xs:tt)*) => (1_usize + $crate::count!($($xs)*));
}

/// Equivalent to `into_vec!` when the inferred type is `Cow<'_, str>`.
///
/// Prefer this over `into_vec!` if you are using Cow<str>, as
/// the compiler currently cannot optimize away the unnecessary
/// drops, so this will have a smaller generated code size than
/// `into_vec!` if you mix `format!` and static strings.
///
/// TODO: implement this optimization in rustc itself, possibly as
/// as a MIR pass.
#[macro_export]
macro_rules! inplace_vec {
  ($($x:expr),* $(,)?) => {{
    use std::mem::{self, MaybeUninit};
    use std::borrow::Cow;

    const SIZE: usize = $crate::count!($($x)*);
    #[allow(unused_assignments)]
    #[allow(clippy::transmute_undefined_repr)]
    #[allow(clippy::macro_metavars_in_unsafe)]
    unsafe {
      let mut v: Vec<MaybeUninit<Cow<_>>> = Vec::with_capacity(SIZE);
      v.set_len(SIZE);

      let mut idx = 0;
      $(
        v[idx] = MaybeUninit::new($x.into());
        idx += 1;
      )*

      mem::transmute::<Vec<MaybeUninit<Cow<_>>>, Vec<Cow<_>>>(v)
    }
  }};
}

#[macro_export]
macro_rules! ref_smallvec {
  ($t:ty, $size:expr, [$($x:expr),* $(,)?]$(,)?) => {{
    let mut sv = SmallVec::<[&$t; $size]>::new_const();

    sv.extend(
      [
        $(
          AsRef::<$t>::as_ref($x),
        )*
      ]
    );

    sv
  }};
}

#[macro_export]
macro_rules! into_vec {
  ($($x:expr),* $(,)?) => {
    vec![
      $(
        $x.into(),
      )*
    ]
  };
}

#[macro_export]
macro_rules! into_array {
  ($($x:expr),* $(,)?) => {
    [
      $(
        $x.into(),
      )*
    ]
  };
}

#[macro_export]
macro_rules! into_smallvec {
  ($($x:expr),* $(,)?) => {
    smallvec::smallvec![
      $(
        $x.into(),
      )*
    ]
  };
}

/// Attempts to create the directory if it does not exist, logging and returning
/// and error if creating the directory failed.
#[macro_export]
macro_rules! create_dir {
    ($loc:expr) => {
        match std::fs::create_dir(&$loc) {
            Ok(()) => Ok(()),
            Err(e) => match e.kind() {
                std::io::ErrorKind::AlreadyExists => Ok(()),
                _ => {
                    error!("Error while creating directory {:?}: {}", &$loc, e);
                    Err(e)
                },
            },
        }
    };
}

#[inline]
pub(crate) fn printable_base10_digits(x: usize) -> u32 {
    (((x as f64).log10() + 1.0).floor() as u32).max(1)
}

/// Reads dir and returns all files
/// Depth 1
#[inline]
pub fn read_in_dir(path: &Path) -> anyhow::Result<impl Iterator<Item = PathBuf>> {
    let dir = std::fs::read_dir(path)?;
    Ok(dir.into_iter().filter_map(Result::ok).filter_map(|d| {
        d.file_type().map_or(None, |file_type| {
            if file_type.is_file() {
                Some(d.path())
            } else {
                None
            }
        })
    }))
}
