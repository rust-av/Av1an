use std::{
    collections::{BTreeMap, HashMap},
    io::{Cursor, Write},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
};

use anyhow::{ensure, Context, Result};
use av_decoders::{Decoder, Ffms2Decoder, VapoursynthDecoder};
pub use av_decoders::{DecoderError, ModifyNode};
use thiserror::Error as ThisError;

use crate::{
    core::input::clip_info::ClipInfo,
    ffmpeg::get_clip_info,
    models::input::{
        ImportMethod,
        Input as InputModel,
        VapourSynthImportMethod,
        VapourSynthScriptSource,
    },
    vapoursynth::{
        get_clip_info as get_vs_clip_info,
        plugins::{
            bestsource::VideoSource as BestSource,
            dgdecodenv::DGSource,
            ffms2::Source as FFMS2,
            lsmash::LWLibavSource,
        },
        VapourSynthError,
    },
};

pub mod clip_info;
pub mod pixel_format;

pub enum Input {
    Video {
        path:          PathBuf,
        import_method: ImportMethod,
        decoder:       Decoder,
        clip_info:     Option<ClipInfo>,
    },
    VapourSynth {
        path:          PathBuf,
        import_method: VapourSynthImportMethod,
        cache_path:    Option<PathBuf>,
        decoder:       Decoder,
        clip_info:     Option<ClipInfo>,
    },
    VapourSynthScript {
        source:    VapourSynthScriptSource,
        variables: HashMap<String, String>,
        index:     u8,
        decoder:   Decoder,
        clip_info: Option<ClipInfo>,
    },
}

impl Input {
    #[inline]
    pub fn as_data(&self) -> InputModel {
        match self {
            Input::Video {
                path,
                import_method,
                ..
            } => InputModel::Video {
                path:          path.clone(),
                import_method: import_method.clone(),
            },
            Input::VapourSynth {
                path,
                import_method,
                cache_path,
                ..
            } => InputModel::VapourSynth {
                path:          path.clone(),
                import_method: import_method.clone(),
                cache_path:    cache_path.clone(),
            },
            Input::VapourSynthScript {
                source,
                variables,
                index,
                ..
            } => InputModel::VapourSynthScript {
                source:    source.clone(),
                variables: variables.clone(),
                index:     *index,
            },
        }
    }

    #[inline]
    pub fn validate(data: &InputModel) -> Result<()> {
        match data {
            InputModel::Video {
                path, ..
            } => {
                ensure!(path.exists(), InputError::VideoFileNotFound(path.clone()));
                if let Some(ext) = path.extension() {
                    ensure!(
                        ext != "vpy" && ext != "py",
                        InputError::NotAVideoFile(path.clone())
                    );
                }
                Ok(())
            },
            InputModel::VapourSynth {
                path, ..
            } => {
                // TODO: Check if VapourSynth + plugin is installed and static cache it
                ensure!(path.exists(), InputError::VideoFileNotFound(path.clone()));
                if let Some(ext) = path.extension() {
                    ensure!(
                        ext != "vpy" && ext != "py",
                        InputError::NotAVideoFile(path.clone())
                    );
                }

                Ok(())
            },
            InputModel::VapourSynthScript {
                source, ..
            } => {
                // TODO: Check if VapourSynth is installed and static cache it
                match source {
                    VapourSynthScriptSource::Path(path) => {
                        ensure!(
                            path.exists(),
                            InputError::VapourSynthScriptNotFound(path.clone())
                        );
                        if let Some(ext) = path.extension() {
                            ensure!(
                                ext == "vpy" || ext == "py",
                                InputError::NotAVapourSynthScript(path.clone())
                            );
                        }
                    },
                    VapourSynthScriptSource::Text(_) => (),
                }

                Ok(())
            },
        }
    }

    #[inline]
    pub fn from_video(data: &InputModel) -> Result<Self> {
        match data {
            InputModel::Video {
                path,
                import_method,
            } => {
                Input::validate(data)?;
                match import_method {
                    // ImportMethod::FFmpeg {} => {
                    //     unimplemented!();
                    // },
                    ImportMethod::FFMS2 {} => {
                        let ffms2_decoder = Ffms2Decoder::new(path)?;
                        let decoder = Decoder::from_decoder_impl(av_decoders::DecoderImpl::Ffms2(
                            ffms2_decoder,
                        ))?;

                        Ok(Input::Video {
                            path: path.clone(),
                            import_method: import_method.clone(),
                            decoder,
                            clip_info: None,
                        })
                    },
                }
            },
            _ => panic!("expected `Input::Video`"),
        }
    }

    #[inline]
    pub fn from_vapoursynth(data: &InputModel, modify_node: Option<ModifyNode>) -> Result<Self> {
        Input::validate(data)?;
        match data {
            InputModel::VapourSynth {
                path,
                import_method,
                cache_path,
            } => {
                let mut vs_decoder = VapoursynthDecoder::new()?;
                let error_handler = || {
                    |error| match error {
                        VapourSynthError::VideoImportError {
                            plugin,
                            message,
                        } => DecoderError::VapoursynthScriptError {
                            cause: format!(
                                "{plugin} failed to import video: {message}",
                                plugin = plugin,
                                message = message
                            ),
                        },
                        _ => DecoderError::GenericDecodeError {
                            cause: "Failed to import video".to_owned(),
                        },
                    }
                };

                let decoder = match import_method {
                    VapourSynthImportMethod::LSMASHWorks {
                        index,
                    } => {
                        let source = path.clone();
                        let cache_path = cache_path.clone();
                        let err_handler = error_handler();
                        let index = *index;
                        let node_modifier: ModifyNode =
                            Box::new(move |core: vapoursynth::core::CoreRef, _node| {
                                let plugin = LWLibavSource {
                                    source: source.clone(),
                                    cachefile: cache_path.clone(),
                                    stream_index: index.map(|i| i as i32),
                                    ..Default::default()
                                };
                                let node = plugin.invoke(core).map_err(err_handler)?;
                                let node = if let Some(modify_node) = &modify_node {
                                    modify_node(core, Some(node))?
                                } else {
                                    node
                                };

                                Ok(node)
                            });

                        vs_decoder.register_node_modifier(node_modifier)?;
                        Decoder::from_decoder_impl(av_decoders::DecoderImpl::Vapoursynth(
                            vs_decoder,
                        ))?
                    },
                    VapourSynthImportMethod::FFMS2 {
                        index,
                    } => {
                        let source = path.clone();
                        let cache_path = cache_path.clone();
                        let err_handler = error_handler();
                        let index = *index;
                        let node_modifier: ModifyNode = Box::new(move |core, _node| {
                            let plugin = FFMS2 {
                                source: source.clone(),
                                cachefile: cache_path.clone(),
                                track: index.map(|i| i as i32),
                                ..Default::default()
                            };
                            let node = plugin.invoke(core).map_err(err_handler)?;
                            let node = if let Some(modify_node) = &modify_node {
                                modify_node(core, Some(node))?
                            } else {
                                node
                            };

                            Ok(node)
                        });

                        vs_decoder.register_node_modifier(node_modifier)?;
                        Decoder::from_decoder_impl(av_decoders::DecoderImpl::Vapoursynth(
                            vs_decoder,
                        ))?
                    },
                    VapourSynthImportMethod::BestSource {
                        index,
                    } => {
                        let source = path.clone();
                        let cache_path = cache_path.clone();
                        let err_handler = error_handler();
                        let index = *index;
                        let node_modifier: ModifyNode = Box::new(move |core, _node| {
                            let plugin = BestSource {
                                source: source.clone(),
                                cachepath: cache_path.clone(),
                                track: index.map(|i| i as i32),
                                ..Default::default()
                            };
                            let node = plugin.invoke(core).map_err(err_handler)?;
                            let node = if let Some(modify_node) = &modify_node {
                                modify_node(core, Some(node))?
                            } else {
                                node
                            };

                            Ok(node)
                        });

                        vs_decoder.register_node_modifier(node_modifier)?;
                        Decoder::from_decoder_impl(av_decoders::DecoderImpl::Vapoursynth(
                            vs_decoder,
                        ))?
                    },
                    VapourSynthImportMethod::DGDecNV {
                        dgindexnv_executable,
                    } => {
                        let source = path.clone();
                        let cache_path = cache_path.clone();
                        let err_handler = error_handler();
                        DGSource::index_video(
                            path,
                            cache_path.as_deref(),
                            dgindexnv_executable.as_deref(),
                        )
                        .map_err(|_| DecoderError::UnsupportedDecoder)?;
                        let node_modifier: ModifyNode = Box::new(move |core, _node| {
                            let plugin = DGSource {
                                source: source.clone(),
                                // Undocumented, needs testing
                                indexing_path: cache_path.clone(),
                                ..Default::default()
                            };
                            let node = plugin.invoke(core).map_err(err_handler)?;
                            let node = if let Some(modify_node) = &modify_node {
                                modify_node(core, Some(node))?
                            } else {
                                node
                            };

                            Ok(node)
                        });

                        vs_decoder.register_node_modifier(node_modifier)?;
                        Decoder::from_decoder_impl(av_decoders::DecoderImpl::Vapoursynth(
                            vs_decoder,
                        ))?
                    },
                };

                Ok(Input::VapourSynth {
                    path: path.clone(),
                    import_method: import_method.clone(),
                    cache_path: cache_path.clone(),
                    // modify_node,
                    decoder,
                    clip_info: None,
                })
            },
            InputModel::VapourSynthScript {
                source,
                variables,
                index,
            } => {
                let mut vs_decoder = match source {
                    VapourSynthScriptSource::Path(path) => {
                        VapoursynthDecoder::from_file(path, variables.clone())?
                    },
                    VapourSynthScriptSource::Text(script) => {
                        VapoursynthDecoder::from_script(script, variables.clone())?
                    },
                };
                if let Some(node_modifier) = modify_node {
                    vs_decoder.register_node_modifier(node_modifier)?;
                }

                let decoder =
                    Decoder::from_decoder_impl(av_decoders::DecoderImpl::Vapoursynth(vs_decoder))?;

                Ok(Input::VapourSynthScript {
                    source: source.clone(),
                    variables: variables.clone(),
                    index: *index,
                    // modify_node,
                    decoder,
                    clip_info: None,
                })
            },
            _ => panic!("expected `Input::VapourSynth` or `Input::VapourSynthScript`"),
        }
    }

    #[inline]
    pub fn from_data(data: &InputModel) -> Result<Self> {
        match data {
            InputModel::Video {
                ..
            } => Input::from_video(data),
            InputModel::VapourSynth {
                ..
            }
            | InputModel::VapourSynthScript {
                ..
            } => Input::from_vapoursynth(data, None),
        }
    }

    #[inline]
    pub fn decoder(&mut self) -> &mut Decoder {
        match self {
            Input::Video {
                decoder, ..
            } => decoder,
            Input::VapourSynth {
                decoder, ..
            } => decoder,
            Input::VapourSynthScript {
                decoder, ..
            } => decoder,
        }
    }

    #[inline]
    pub fn clip_info(&mut self) -> Result<ClipInfo> {
        let clip_info = match self {
            Input::Video {
                path,
                clip_info,
                ..
            } => {
                // Ideally, these values are retrieved via VideoDetails instead
                if clip_info.is_none() {
                    let info = get_clip_info(path.as_path())?;
                    *clip_info = Some(info);
                }

                clip_info.as_ref()
            },
            Input::VapourSynth {
                decoder,
                clip_info,
                ..
            }
            | Input::VapourSynthScript {
                decoder,
                clip_info,
                ..
            } => {
                if clip_info.is_none() {
                    let info = {
                        let node = decoder.get_vapoursynth_node()?;
                        get_vs_clip_info(&node)?
                    };
                    *clip_info = Some(info);
                }

                clip_info.as_ref()
            },
        };

        Ok(*clip_info.expect("ClipInfo is Some"))
    }

    #[inline]
    pub fn y4m_header(&mut self, frames: Option<usize>) -> Result<String> {
        let clip_info = self.clip_info()?;
        let decoder = match self {
            Input::Video {
                decoder, ..
            } => decoder,
            Input::VapourSynth {
                decoder, ..
            } => decoder,
            Input::VapourSynthScript {
                decoder, ..
            } => decoder,
        };
        let details = decoder.get_video_details();

        let chroma_str = match details.chroma_sampling {
            av_decoders::v_frame::chroma::ChromaSubsampling::Monochrome => "mono",
            av_decoders::v_frame::chroma::ChromaSubsampling::Yuv420 => "420",
            av_decoders::v_frame::chroma::ChromaSubsampling::Yuv422 => "422",
            av_decoders::v_frame::chroma::ChromaSubsampling::Yuv444 => "444",
        };
        let chroma_header = format!(
            "{}{}{}",
            chroma_str,
            match details.chroma_sampling {
                av_decoders::v_frame::chroma::ChromaSubsampling::Monochrome => "",
                _ if details.bit_depth > 8 => "p",
                _ => "",
            },
            match details.bit_depth {
                _ if details.bit_depth > 8 => format!("{}", details.bit_depth),
                _ => String::new(),
            },
        );

        let header = format!(
            "YUV4MPEG2 C{} W{} H{} F{}:{} Ip A0:0{}\n",
            chroma_header,
            clip_info.resolution.0,
            clip_info.resolution.1,
            clip_info.frame_rate.numer(),
            clip_info.frame_rate.denom(),
            frames.map_or_else(String::new, |frames| format!(" XLENGTH {}", frames))
        );

        Ok(header)
    }

    #[inline]
    pub fn y4m_frame(&mut self, index: usize) -> Result<Cursor<Vec<u8>>> {
        static FRAME_HEADER: &str = "FRAME\n";
        static CONTEXT: &str = "get y4m frame";

        let mut stream = Cursor::new(Vec::new());
        stream.write_all(FRAME_HEADER.as_bytes())?; // Frame header

        match self {
            Input::Video {
                decoder, ..
            } => {
                let details = decoder.get_video_details();
                match details.bit_depth {
                    8 => {
                        let frame = decoder.get_video_frame::<u8>(index).context(CONTEXT)?;

                        let mut planes = Vec::new();
                        planes.extend(frame.y_plane.byte_data());
                        if let Some(plane) = frame.u_plane {
                            planes.extend(plane.byte_data());
                        }
                        if let Some(plane) = frame.v_plane {
                            planes.extend(plane.byte_data());
                        }
                        stream.write_all(&planes)?;
                    },
                    _ => {
                        let frame = decoder.get_video_frame::<u16>(index).context(CONTEXT)?;

                        let mut planes = Vec::new();
                        planes.extend(frame.y_plane.byte_data());
                        if let Some(plane) = frame.u_plane {
                            planes.extend(plane.byte_data());
                        }
                        if let Some(plane) = frame.v_plane {
                            planes.extend(plane.byte_data());
                        }
                        stream.write_all(&planes)?;
                    },
                }
            },
            Input::VapourSynth {
                decoder, ..
            }
            | Input::VapourSynthScript {
                decoder, ..
            } => {
                let node = decoder.get_vapoursynth_node()?;
                let frame = node.get_frame(index)?;
                let framedata = {
                    let mut data = Vec::new();
                    let planes_indices =
                        if frame.format().color_family() == vapoursynth::format::ColorFamily::RGB {
                            [1, 2, 0]
                        } else {
                            [0, 1, 2]
                        };
                    for plane_index in planes_indices {
                        if let Ok(plane_data) = frame.data(plane_index) {
                            data.extend_from_slice(plane_data);
                        } else {
                            for row in 0..frame.height(plane_index) {
                                data.extend_from_slice(frame.data_row(plane_index, row));
                            }
                        }
                    }
                    data
                };

                stream.write_all(&framedata)?;
            },
        }

        Ok(stream)
    }

    #[inline]
    pub fn y4m_frames(
        &mut self,
        // frame_sender: mpsc::Sender<Cursor<Vec<u8>>>,
        frame_sender: crossbeam_channel::Sender<Cursor<Vec<u8>>>,
        start: usize,
        end: usize,
    ) -> Result<()> {
        static FRAME_HEADER: &str = "FRAME\n";
        static CONTEXT: &str = "get y4m frame";
        // let mut stream = Cursor::new(Vec::new());
        let bit_depth = self.clip_info()?.format_info.as_bit_depth()?;

        match self {
            Input::Video {
                decoder, ..
            } => {
                // let details = decoder.get_video_details();
                for index in start..end {
                    // let framedata = match details.bit_depth {
                    let framedata = match bit_depth {
                        8 => {
                            let frame = decoder.get_video_frame::<u8>(index).context(CONTEXT)?;

                            let mut planes = Vec::new();
                            planes.extend(frame.y_plane.byte_data());
                            if let Some(plane) = frame.u_plane {
                                planes.extend(plane.byte_data());
                            }
                            if let Some(plane) = frame.v_plane {
                                planes.extend(plane.byte_data());
                            }
                            planes
                        },
                        _ => {
                            let frame = decoder.get_video_frame::<u16>(index).context(CONTEXT)?;

                            let mut planes = Vec::new();
                            planes.extend(frame.y_plane.byte_data());
                            if let Some(plane) = frame.u_plane {
                                planes.extend(plane.byte_data());
                            }
                            if let Some(plane) = frame.v_plane {
                                planes.extend(plane.byte_data());
                            }
                            planes
                        },
                    };
                    let mut cursor = Cursor::new(Vec::new());
                    cursor.write_all(FRAME_HEADER.as_bytes())?;
                    cursor.write_all(&framedata)?;
                    frame_sender.send(cursor)?;
                }
            },
            Input::VapourSynth {
                decoder, ..
            }
            | Input::VapourSynthScript {
                decoder, ..
            } => {
                let node = decoder.get_vapoursynth_node()?;
                let pair = Arc::new((Mutex::new(BTreeMap::new()), Condvar::new()));
                for index in start..end {
                    let pair_clone = Arc::clone(&pair);
                    node.get_frame_async(index, move |frame, _index, _node| {
                        let frame = frame.expect("Failed to get frame");
                        let (lock, condvar) = &*pair_clone;
                        let mut map = lock.lock().expect("mutex should acquire lock");
                        map.insert(index, frame);
                        condvar.notify_one();
                    });
                }

                let mut next_frame_index = start;

                while next_frame_index < end {
                    let (map, condvar) = &*pair;
                    let map = map.lock().expect("mutex should acquire lock");
                    let mut map = condvar
                        .wait_while(map, |m| !m.contains_key(&next_frame_index))
                        .expect("Condvar should be notified");

                    let (_index, frame) = map.pop_first().expect("Map should have frame");
                    next_frame_index += 1;
                    drop(map);
                    let framedata = {
                        let mut data = Vec::new();
                        data.extend_from_slice(FRAME_HEADER.as_bytes());
                        let planes_indices = if frame.format().color_family()
                            == vapoursynth::format::ColorFamily::RGB
                        {
                            [1, 2, 0]
                        } else {
                            [0, 1, 2]
                        };
                        for plane_index in planes_indices {
                            if let Ok(plane_data) = frame.data(plane_index) {
                                data.extend_from_slice(plane_data);
                            } else {
                                for row in 0..frame.height(plane_index) {
                                    data.extend_from_slice(frame.data_row(plane_index, row));
                                }
                            }
                        }
                        data
                    };

                    frame_sender.send(Cursor::new(framedata))?;
                }
            },
        }

        drop(frame_sender);

        Ok(())
    }

    // #[inline]
    // pub fn y4m_frames_node(node: Node, frame_sender:
    // mpsc::Sender<Cursor<Vec<u8>>>, start: usize, end: usize) -> Result<()> {

    // }
}

#[derive(Debug, ThisError)]
pub enum InputError {
    #[error("No video file found at {0}")]
    VideoFileNotFound(PathBuf),
    #[error("No VapourSynth script file found at {0}")]
    VapourSynthScriptNotFound(PathBuf),
    #[error("File {0} is not a video file")]
    NotAVideoFile(PathBuf),
    #[error("File {0} is not a VapourSynth script")]
    NotAVapourSynthScript(PathBuf),
}
