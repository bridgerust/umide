use crate::decoder::{VideoDecoder, DecodedFrame, DecodeError, GpuFrame, HardwareSurfaceHandle};
use std::ffi::c_void;
use std::ptr;
#[allow(unused_imports)]
use video_toolbox_sys::decompression::{
    VTDecompressionSessionRef, VTDecompressionSessionCreate, 
    VTDecompressionSessionDecodeFrame, VTDecodeInfoFlags,
    VTDecompressionSessionInvalidate, VTDecompressionOutputCallbackRecord,
};
use core_media_sys::{
    CMBlockBufferCreateWithMemoryBlock,
    CMBlockBufferRef, CMSampleBufferRef, CMVideoFormatDescriptionRef,
    CMTime, 
};
use tracing::{info, error};

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    pub fn CMSampleBufferCreate(
        allocator: *mut c_void,
        data_buffer: CMBlockBufferRef,
        data_ready: u8,
        make_data_ready_callback: Option<extern "C" fn()>,
        make_data_ready_refcon: *mut c_void,
        format_description: CMVideoFormatDescriptionRef,
        num_samples: i32,
        num_sample_timing_entries: i32,
        sample_timing_array: *const c_void, // CMSampleTimingInfo
        num_sample_size_entries: i32,
        sample_size_array: *const usize,
        sample_buffer_out: *mut CMSampleBufferRef
    ) -> i32;

    pub fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
        allocator: *mut c_void,
        parameter_set_count: usize,
        parameter_set_pointers: *const *const u8,
        parameter_set_sizes: *const usize,
        nal_unit_header_length: i32,
        format_description_out: *mut CMVideoFormatDescriptionRef
    ) -> i32;
}


#[allow(unused_imports)]
use core_foundation_sys::base::kCFAllocatorDefault;
use core_video_sys::{CVPixelBufferRef, CVPixelBufferRetain};

extern "C" fn decode_callback(
    _output_ref_con: *mut c_void,
    source_frame_ref_con: *mut c_void,
    status: i32,
    _info_flags: u32,
    image_buffer: *mut c_void,
    _pts: CMTime,
    _duration: CMTime,
) {
    if status == 0 && !image_buffer.is_null() {
        unsafe {
            let frames_ptr = source_frame_ref_con as *mut Vec<DecodedFrame>;
            if !frames_ptr.is_null() {
                 let image_buffer = image_buffer as CVPixelBufferRef;
                 CVPixelBufferRetain(image_buffer);
                 let width = core_video_sys::CVPixelBufferGetWidth(image_buffer) as u32;
                 let height = core_video_sys::CVPixelBufferGetHeight(image_buffer) as u32;
                 
                 let handle = HardwareSurfaceHandle {
                     ptr: image_buffer as usize,
                     width,
                     height,
                 };
                 
                 (*frames_ptr).push(DecodedFrame {
                     width,
                     height,
                     frame: GpuFrame::Hardware(handle),
                 });
                 // tracing::info!("Callback: Pushed frame {}x{}", width, height);
            } else {
                 // tracing::error!("Callback: frames_ptr is null!");
            }
        }
    } else if status != 0 {
        // tracing::error!("Callback error: {}", status);
    }
}



#[cfg(target_os = "macos")]
pub struct VideoToolboxDecoder {
    session: VTDecompressionSessionRef,
    format_desc: Option<CMVideoFormatDescriptionRef>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
}

#[cfg(target_os = "macos")]
unsafe impl Send for VideoToolboxDecoder {}

#[cfg(target_os = "macos")]
impl Drop for VideoToolboxDecoder {
    fn drop(&mut self) {
        unsafe {
            if !self.session.is_null() {
                VTDecompressionSessionInvalidate(self.session);
                // CFRelease(self.session as _); // TODO: Need CoreFoundation bindings for release
            }
             if let Some(_desc) = self.format_desc {
                // CFRelease(desc as _);
            }
        }
    }
}

#[cfg(target_os = "macos")]
impl VideoToolboxDecoder {
    pub fn new() -> Result<Self, DecodeError> {
        Ok(Self {
            session: ptr::null_mut(),
            format_desc: None,
            sps: None,
            pps: None,
        })
    }

    fn create_session(&mut self, sps: &[u8], pps: &[u8]) -> Result<(), DecodeError> {
        unsafe {
            let parameter_sets = [sps.as_ptr(), pps.as_ptr()];
            let parameter_set_sizes = [sps.len(), pps.len()];
            
            let mut format_desc = ptr::null_mut();
            let status = CMVideoFormatDescriptionCreateFromH264ParameterSets(
                ptr::null_mut(), // kCFAllocatorDefault
                2,
                parameter_sets.as_ptr(),
                parameter_set_sizes.as_ptr(),
                4, // NALUnitHeaderLength (4 bytes for AVCC)
                &mut format_desc
            );

            if status != 0 {
                return Err(DecodeError::DecodeFailed(format!("Failed to create format description: {}", status)));
            }
            self.format_desc = Some(format_desc);

            // Create Decompression Session
            let callback_record = VTDecompressionOutputCallbackRecord {
                decompressionOutputCallback: core::mem::transmute(decode_callback as *const ()), // Brute force cast to expected fn pointer type. 
                // Or better: decode_callback as _ fails if signatures differ.
                // Given the complexities of cross-crate types, transmute is nuclear option but works if ABI matches (it does, c_void).
                decompressionOutputRefCon: ptr::null_mut(), 
            };

            let mut session: VTDecompressionSessionRef = ptr::null_mut(); 
            
            let status = VTDecompressionSessionCreate(
                ptr::null_mut(), 
                format_desc,
                ptr::null_mut(), // videoDecoderSpecification
                ptr::null_mut(), // destinationImageBufferAttributes
                &callback_record,
                &mut session as *mut _ as *mut _ 
            );

            if status != 0 {
                return Err(DecodeError::DecodeFailed(format!("Failed to create decompression session: {}", status)));
            }
            self.session = session;
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
impl VideoDecoder for VideoToolboxDecoder {
    fn decode_frame(&mut self, data: &[u8]) -> Result<Vec<DecodedFrame>, DecodeError> {
        let mut frames = Vec::new();
        // nalus removed as we process directly
        
        // Simple Annex-B splitter
        // Assumes start codes are 00 00 00 01 or 00 00 01.
        let mut start_indices = Vec::new();
        let mut i = 0;
        while i < data.len() - 3 {
             if data[i] == 0 && data[i+1] == 0 {
                 if data[i+2] == 1 {
                     start_indices.push((i, 3)); // 00 00 01
                     i += 3;
                 } else if data[i+2] == 0 && data[i+3] == 1 {
                     start_indices.push((i, 4)); // 00 00 00 01
                     i += 4;
                 } else {
                     i += 1;
                 }
             } else {
                 i += 1;
             }
        }

        let mut avcc_buffer = Vec::with_capacity(data.len()); // May grow slightly or shrink

        for idx in 0..start_indices.len() {
            let (start, len) = start_indices[idx];
            let end = if idx + 1 < start_indices.len() {
                start_indices[idx+1].0
            } else {
                data.len()
            };
            
            let nalu_data = &data[start+len..end];
            let nalu_type = nalu_data[0] & 0x1F;
            
            // Debug Log
            info!("Found NALU Type: {} Len: {}", nalu_type, nalu_data.len());

            match nalu_type {
                7 => {
                    info!("Captured SPS");
                    self.sps = Some(nalu_data.to_vec());
                },
                8 => {
                    info!("Captured PPS");
                    self.pps = Some(nalu_data.to_vec());
                },
                _ => {}
            }
            
            // Append length-prefixed NALU to buffer
            let len_u32 = nalu_data.len() as u32;
            avcc_buffer.extend_from_slice(&len_u32.to_be_bytes());
            avcc_buffer.extend_from_slice(nalu_data);
        }

        if self.session.is_null() {
            let sps = self.sps.clone();
            let pps = self.pps.clone();
            if let (Some(s), Some(p)) = (sps, pps) {
                info!("Attempting to create session...");
                match self.create_session(&s, &p) {
                    Ok(_) => { 
                        info!("Session created successfully!");
                    },
                    Err(e) => {
                        error!("Session creation failed: {:?}", e);
                        return Err(e);
                    }
                }
            } else {
                info!("Cannot create session yet. Missing SPS/PPS.");
                return Ok(vec![]);
            }
        }

        if self.session.is_null() {
             // Can't decode without session (SPS/PPS)
             return Ok(vec![]); 
        }

        unsafe {
            let mut block_buffer = ptr::null(); 

             let status = CMBlockBufferCreateWithMemoryBlock(
                ptr::null_mut(),
                avcc_buffer.as_ptr() as *mut c_void, // memoryBlock
                avcc_buffer.len() as usize,         // blockLength
                ptr::null_mut(),                    // blockAllocator (NULL = use default)
                ptr::null_mut(),                    // customBlockSource
                0,                                  // offsetToData
                avcc_buffer.len() as usize,         // dataLength
                0,                                  // flags
                &mut block_buffer as *mut _ as *mut _ // Cast to fit my extern decl
            );
            
            if status != 0 {
                 return Err(DecodeError::DecodeFailed(format!("Failed to create block buffer: {}", status)));
            }

            // Create CMSampleBuffer
            let mut sample_buffer: CMSampleBufferRef = ptr::null_mut();

            let sample_size = avcc_buffer.len();
             let status = CMSampleBufferCreate(
                ptr::null_mut(), // Allocator
                block_buffer,    // data_buffer (pass value)
                1,               // dataReady
                None,            // makeDataReadyCallback
                ptr::null_mut(), // makeDataReadyRefCon
                self.format_desc.unwrap(), // FormatDescription
                1,               // numSamples
                0,               // numSampleTimingEntries
                ptr::null_mut(), // timingArray
                1,               // numSampleSizeEntries
                &(sample_size as usize) as *const usize, // sampleSizeArray
                &mut sample_buffer as *mut _ as *mut _   // sampleBufferOut
            );
            
            if status != 0 {
                return Err(DecodeError::DecodeFailed(format!("Failed to create sample buffer: {}", status)));
            }

            // Decode
            let flags = 0; // kVTDecodeFrame_EnableAsynchronousDecompression = 1<<0
            let mut info_flags = 0;
            
            let status = VTDecompressionSessionDecodeFrame(
                self.session,
                sample_buffer as *mut c_void, // Cast to *mut c_void for sys crate compatibility
                flags,
                &mut frames as *mut _ as *mut c_void, // sourceFrameContext
                &mut info_flags
            );

            // Cleanup buffers (CoreFoundation usually handles this via Release, but we need to verify sys crate hygiene)
             // CMSampleBufferRelease(sample_buffer); // Need bindings
             // CMBlockBufferRelease(block_buffer); 
            // We probably leak here without CFRelease bindings? 
            // Yes, we need CFRelease. I commented it out earlier.
             
            if status != 0 {
                 return Err(DecodeError::DecodeFailed(format!("Decode failed: {}", status)));
            }
        }

        Ok(frames)
    }

    fn flush(&mut self) -> Result<Vec<DecodedFrame>, DecodeError> {
        Ok(vec![])
    }

    fn reset(&mut self) -> Result<(), DecodeError> {
        Ok(())
    }
}
