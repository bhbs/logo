mod crc32 {
    // https://github.com/ledbettj/crc32/blob/master/rust/src/crc32.rs
    pub struct Crc32 {
        table: [u32; 256],
        value: u32,
    }

    const CRC32_INITIAL: u32 = 0xedb88320;

    impl Crc32 {
        pub fn new() -> Crc32 {
            let mut c = Crc32 {
                table: [0; 256],
                value: 0xffffffff,
            };
            for i in 0..256 {
                let mut v = i as u32;
                for _ in 0..8 {
                    v = if v & 1 != 0 {
                        CRC32_INITIAL ^ (v >> 1)
                    } else {
                        v >> 1
                    }
                }
                c.table[i] = v;
            }
            c
        }

        pub fn start(&mut self) {
            self.value = 0xffffffff;
        }

        pub fn update(&mut self, buf: &[u8]) {
            for &i in buf {
                self.value =
                    self.table[((self.value ^ (i as u32)) & 0xff) as usize] ^ (self.value >> 8);
            }
        }

        pub fn finalize(&mut self) -> u32 {
            self.value ^ 0xffffffff_u32
        }

        #[allow(dead_code)]
        pub fn crc(&mut self, buf: &[u8]) -> u32 {
            self.start();
            self.update(buf);
            self.finalize()
        }
    }
}

mod adler32 {
    // https://en.wikipedia.org/wiki/Adler-32

    pub struct Adler32 {
        a: u32,
        b: u32,
    }

    const MOD_ADLER: u32 = 65521;

    impl Adler32 {
        pub fn new() -> Adler32 {
            Adler32 { a: 1, b: 0 }
        }

        pub fn start(&mut self) {
            self.a = 1;
            self.b = 0;
        }

        pub fn update(&mut self, buf: &[u8]) {
            for &i in buf {
                self.a = (self.a + i as u32) % MOD_ADLER;
                self.b = (self.a + self.b) % MOD_ADLER;
            }
        }

        pub fn finalize(&self) -> u32 {
            (self.b << 16) | self.a
        }

        #[allow(dead_code)]
        pub fn crc(&mut self, buf: &[u8]) -> u32 {
            self.start();
            self.update(buf);
            self.finalize()
        }
    }
}

// big endian
#[inline]
fn u32_to_u8_be(v: u32) -> [u8; 4] {
    [(v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8]
}

mod fake_zlib {
    use super::adler32;
    use super::u32_to_u8_be;

    // Use 'none' compression
    pub fn compress(data: &[u8]) -> Vec<u8> {
        const CHUNK_SIZE: usize = 65530;

        let final_len =
            // header
            2 +
            // every chunk adds 5 bytes [1:type, 4:size].
            (5 * {
                let n = data.len() / CHUNK_SIZE;
                // include an extra chunk when we don't fit exactly into CHUNK_SIZE
                n + {usize::from(data.len() != n * CHUNK_SIZE || data.is_empty())}
            }) +
            // data
            data.len() +
            // crc
            4
        ;

        let mut raw_data = Vec::with_capacity(final_len);
        // header
        raw_data.extend([120, 1]);
        let mut pos_curr = 0_usize;
        let mut crc = adler32::Adler32::new();
        loop {
            let pos_next = ::std::cmp::min(data.len(), pos_curr + CHUNK_SIZE);
            let chunk_len = (pos_next - pos_curr) as u32;
            let is_last = pos_next == data.len();
            raw_data.extend([
                // type
                u8::from(is_last),
                // size
                (chunk_len & 0xff) as u8,
                ((chunk_len >> 8) & 0xff) as u8,
                (0xff - (chunk_len & 0xff)) as u8,
                (0xff - ((chunk_len >> 8) & 0xff)) as u8,
            ]);

            raw_data.extend(&data[pos_curr..pos_next]);

            crc.update(&data[pos_curr..pos_next]);

            if is_last {
                break;
            }
            pos_curr = pos_next;
        }

        raw_data.extend(u32_to_u8_be(crc.finalize()));

        assert_eq!(final_len, raw_data.len());
        raw_data
    }
}

///
/// Write RGBA pixels to uncompressed PNG.
///
pub fn write<W: ::std::io::Write>(
    file: &mut W,
    image: &[u8],
    width: u32,
    height: u32,
) -> Result<(), ::std::io::Error> {
    assert!(width * height * 4 == image.len() as u32);

    fn png_pack<W: ::std::io::Write>(
        file: &mut W,
        png_tag: &[u8; 4],
        data: &[u8],
    ) -> Result<(), ::std::io::Error> {
        file.write_all(&u32_to_u8_be(data.len() as u32))?;
        file.write_all(png_tag)?;
        file.write_all(data)?;
        {
            let mut crc = crc32::Crc32::new();
            crc.start();
            crc.update(png_tag);
            crc.update(data);
            file.write_all(&u32_to_u8_be(crc.finalize()))?;
        }
        Ok(())
    }

    file.write_all(b"\x89PNG\r\n\x1a\n")?;
    {
        let wb = u32_to_u8_be(width);
        let hb = u32_to_u8_be(height);
        let data = [
            wb[0], wb[1], wb[2], wb[3], hb[0], hb[1], hb[2], hb[3], 8, 6, 0, 0, 0,
        ];
        png_pack(file, b"IHDR", &data)?;
    }

    {
        let width_byte_4 = width * 4;
        let final_len = (width_byte_4 + 1) * height;
        let mut raw_data = Vec::with_capacity(final_len as usize);
        let mut span: u32 = (height - 1) * width_byte_4;
        loop {
            raw_data.push(0);
            raw_data.extend(&image[(span as usize)..(span + width_byte_4) as usize]);
            if span == 0 {
                break;
            }
            span -= width_byte_4;
        }
        assert!(final_len == (raw_data.len() as u32));

        png_pack(file, b"IDAT", &fake_zlib::compress(&raw_data))?;
    }

    png_pack(file, b"IEND", &[])?;

    Ok(())
}

fn main() {
    let mut file = std::fs::File::create("target/test.png").unwrap();
    let image_width = 2;
    let image_height = 2;
    let image = vec![
        // R     G     B     A
        0x00, 0x00, 0x00, 0xff, //
        0x10, 0x30, 0x50, 0xff, //
        //
        0x10, 0x30, 0x50, 0xff, //
        0x00, 0x00, 0x00, 0x00, //
    ];

    match write(&mut file, &image, image_width, image_height) {
        Ok(_) => println!("Written image!"),
        Err(e) => println!("Error {:?}", e),
    }
}
