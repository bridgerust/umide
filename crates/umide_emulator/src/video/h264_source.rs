use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

pub struct H264FileSource {
    data: Vec<u8>,
    cursor: usize,
}

impl H264FileSource {
    pub fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        Ok(Self { data, cursor: 0 })
    }

    /// Returns the next NALU (including start code) or None if EOF.
    /// This is a simplified parser: it assumes standard start codes.
    /// Ideally we should bundle all NALUs belonging to one Access Unit (Frame),
    /// but for testing, feeding NALU-by-NALU usually works with VideoToolbox
    /// provided we bundle SPS/PPS/IDR if they are adjacent?
    /// Actually, our decoder implementation accumulates all input data into one SampleBuffer.
    /// So if we pass SPS+PPS+IDR in one call, it creates one SampleBuffer with 3 items.
    /// If we pass them separately, it creates 3 SampleBuffers.
    /// VideoToolbox might complain if we send a SampleBuffer with JUST SPS.
    /// So we should try to group them.
    /// A simple heuristic: Split on "AUD" (Access Unit Delimiter) or VCL NALUs?
    /// Let's stick to NALU-by-NALU for simplicity first. If it fails, we'll improve grouping.
    pub fn next_nalu(&mut self) -> Option<&[u8]> {
        if self.cursor >= self.data.len() {
            return None;
        }

        // Find next start code from cursor + 1 (so we don't match current start code)
        let mut end = self.data.len();
        let mut i = self.cursor + 3; // Skip minimal start code 00 00 01
        while i < self.data.len() - 3 {
            if self.data[i] == 0 && self.data[i + 1] == 0 {
                if self.data[i + 2] == 1 {
                    end = i; // Found 00 00 01
                    break;
                } else if self.data[i + 2] == 0 && self.data[i + 3] == 1 {
                    end = i; // Found 00 00 00 01
                    break;
                }
            }
            i += 1;
        }

        let chunk = &self.data[self.cursor..end];
        self.cursor = end;
        Some(chunk)
    }
}
