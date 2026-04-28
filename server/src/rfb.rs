/// RFB Protocol constants and types

// RFB version
pub const RFB_VERSION: &[u8] = b"RFB 003.008\n";

// Security types
pub const SEC_NONE: u8 = 1;

// Client message types
pub const MSG_SET_PIXEL_FORMAT: u8 = 0;
pub const MSG_SET_ENCODINGS: u8 = 2;
pub const MSG_FB_UPDATE_REQUEST: u8 = 3;
pub const MSG_KEY_EVENT: u8 = 4;
pub const MSG_POINTER_EVENT: u8 = 5;
pub const MSG_CLIENT_CUT_TEXT: u8 = 6;

// Server message types
pub const MSG_FB_UPDATE: u8 = 0;

// Encoding types
pub const ENC_RAW: i32 = 0;
pub const ENC_ZRLE: i32 = 16;
pub const ENC_CURSOR: i32 = -239;
pub const ENC_DESKTOP_SIZE: i32 = -223;

#[derive(Debug, Clone, Copy)]
pub struct PixelFormat {
    pub bits_per_pixel: u8,
    pub depth: u8,
    pub big_endian: bool,
    pub true_color: bool,
    pub red_max: u16,
    pub green_max: u16,
    pub blue_max: u16,
    pub red_shift: u8,
    pub green_shift: u8,
    pub blue_shift: u8,
}

impl PixelFormat {
    pub fn default_32bit() -> Self {
        Self {
            bits_per_pixel: 32,
            depth: 24,
            big_endian: false,
            true_color: true,
            red_max: 255,
            green_max: 255,
            blue_max: 255,
            red_shift: 16,
            green_shift: 8,
            blue_shift: 0,
        }
    }

    pub fn to_bytes(&self) -> [u8; 16] {
        let mut buf = [0u8; 16];
        buf[0] = self.bits_per_pixel;
        buf[1] = self.depth;
        buf[2] = self.big_endian as u8;
        buf[3] = self.true_color as u8;
        buf[4..6].copy_from_slice(&self.red_max.to_be_bytes());
        buf[6..8].copy_from_slice(&self.green_max.to_be_bytes());
        buf[8..10].copy_from_slice(&self.blue_max.to_be_bytes());
        buf[10] = self.red_shift;
        buf[11] = self.green_shift;
        buf[12] = self.blue_shift;
        // 13..16 padding
        buf
    }

    pub fn from_bytes(buf: &[u8; 16]) -> Self {
        Self {
            bits_per_pixel: buf[0],
            depth: buf[1],
            big_endian: buf[2] != 0,
            true_color: buf[3] != 0,
            red_max: u16::from_be_bytes([buf[4], buf[5]]),
            green_max: u16::from_be_bytes([buf[6], buf[7]]),
            blue_max: u16::from_be_bytes([buf[8], buf[9]]),
            red_shift: buf[10],
            green_shift: buf[11],
            blue_shift: buf[12],
        }
    }
}
