use std::mem::ManuallyDrop;

use anyhow::{Context, Result};
use windows::core::Interface;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Variant::VARIANT;

pub struct Packet {
    pub data: Vec<u8>,
    pub key: bool,
}

/// H.264 encoder on Media Foundation. It uses Microsoft's software encoder,
/// which is on every Windows client edition, the hardware encoders come later.
pub struct Encoder {
    transform: IMFTransform,
    codec_api: Option<ICodecAPI>,
    fps: u32,
    output_size: u32,
    provides_samples: bool,
}

fn pack(high: u32, low: u32) -> u64 {
    ((high as u64) << 32) | low as u64
}

impl Encoder {
    pub fn new(width: u32, height: u32, fps: u32, bitrate: u32) -> Result<Self> {
        unsafe {
            let transform: IMFTransform =
                CoCreateInstance(&CLSID_MSH264EncoderMFT, None, CLSCTX_INPROC_SERVER)
                    .context("failed to create the H.264 encoder")?;

            let codec_api = transform.cast::<ICodecAPI>().ok();
            if let Some(api) = &codec_api {
                let _ = api.SetValue(&CODECAPI_AVLowLatencyMode, &VARIANT::from(true));
                let _ = api.SetValue(
                    &CODECAPI_AVEncCommonRateControlMode,
                    &VARIANT::from(eAVEncCommonRateControlMode_CBR.0 as u32),
                );
                let _ = api.SetValue(&CODECAPI_AVEncMPVGOPSize, &VARIANT::from(fps * 10));
            }

            let output = MFCreateMediaType()?;
            output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
            output.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
            output.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            output.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            output.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)?;
            transform
                .SetOutputType(0, &output, 0)
                .context("the H.264 encoder refused the output type")?;

            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            input.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
            input.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            transform
                .SetInputType(0, &input, 0)
                .context("the H.264 encoder refused the input type")?;

            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

            let info = transform.GetOutputStreamInfo(0)?;
            let provides_samples = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32
                    | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32)
                != 0;

            Ok(Encoder {
                transform,
                codec_api,
                fps,
                output_size: info.cbSize.max(width * height),
                provides_samples,
            })
        }
    }

    /// Encodes one NV12 frame, `timestamp` is in 100 ns units.
    pub fn encode(&mut self, nv12: &[u8], timestamp: i64, key: bool) -> Result<Vec<Packet>> {
        unsafe {
            if key {
                if let Some(api) = &self.codec_api {
                    let _ = api.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &VARIANT::from(1u32));
                }
            }

            let buffer = MFCreateMemoryBuffer(nv12.len() as u32)?;
            let mut ptr = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None)?;
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), ptr, nv12.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(nv12.len() as u32)?;

            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(timestamp)?;
            sample.SetSampleDuration(10_000_000 / self.fps as i64)?;

            let mut packets = Vec::new();
            if let Err(err) = self.transform.ProcessInput(0, &sample, 0) {
                if err.code() != MF_E_NOTACCEPTING {
                    return Err(err.into());
                }
                self.drain(&mut packets)?;
                self.transform.ProcessInput(0, &sample, 0)?;
            }
            self.drain(&mut packets)?;
            Ok(packets)
        }
    }

    fn drain(&mut self, packets: &mut Vec<Packet>) -> Result<()> {
        unsafe {
            loop {
                let sample = if self.provides_samples {
                    None
                } else {
                    let sample = MFCreateSample()?;
                    let buffer = MFCreateMemoryBuffer(self.output_size)?;
                    sample.AddBuffer(&buffer)?;
                    Some(sample)
                };

                let mut output = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(sample),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let mut status = 0u32;
                let result = self.transform.ProcessOutput(0, &mut output, &mut status);
                let sample = ManuallyDrop::take(&mut output[0].pSample);
                drop(ManuallyDrop::take(&mut output[0].pEvents));

                match result {
                    Ok(()) => {}
                    Err(err) if err.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                    Err(err) if err.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                        let media_type = self.transform.GetOutputAvailableType(0, 0)?;
                        self.transform.SetOutputType(0, &media_type, 0)?;
                        continue;
                    }
                    Err(err) => return Err(err.into()),
                }

                let Some(sample) = sample else { continue };
                let buffer = sample.ConvertToContiguousBuffer()?;
                let mut ptr = std::ptr::null_mut();
                let mut len = 0u32;
                buffer.Lock(&mut ptr, None, Some(&mut len))?;
                let data = std::slice::from_raw_parts(ptr, len as usize).to_vec();
                buffer.Unlock()?;

                let key = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0
                    || crate::h264::is_key_frame(&data);
                if !data.is_empty() {
                    packets.push(Packet { data, key });
                }
            }
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    #[test]
    fn encodes_key_frame_then_delta_frames() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_FULL).unwrap();
        }
        let (width, height) = (1920usize, 1080usize);
        let mut encoder = Encoder::new(width as u32, height as u32, 30, 2_500_000).unwrap();
        let mut frame = vec![128u8; width * height * 3 / 2];
        let mut kinds = String::new();
        let mut first_output = None;
        let start = std::time::Instant::now();
        for i in 0..60usize {
            for y in 0..height {
                for x in 0..width {
                    frame[y * width + x] = ((x + y + i * 8) % 256) as u8;
                }
            }
            let packets = encoder
                .encode(&frame, (i as i64) * 333_333, i == 0)
                .unwrap();
            if !packets.is_empty() && first_output.is_none() {
                first_output = Some(i);
            }
            for p in packets {
                kinds.push(if p.key { 'K' } else { 'd' });
            }
        }
        println!(
            "first output after input {:?}, {:?} per frame, {kinds}",
            first_output,
            start.elapsed() / 60
        );
        assert!(kinds.starts_with('K'));
        assert!(kinds.contains('d'));
    }
}
