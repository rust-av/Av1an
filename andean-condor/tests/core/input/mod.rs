mod frame_reader;
mod y4m;

use andean_condor::{
    core::input::{DecoderError, Input, ModifyNode},
    models::input::{ImportMethod, Input as InputModel, VapourSynthScriptSource},
};
use anyhow::Result;
use av_decoders::v_frame::chroma::ChromaSubsampling;
use std::{collections::HashMap, io::Cursor, sync::atomic::AtomicBool};
