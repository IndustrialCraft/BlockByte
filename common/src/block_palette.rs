/*pub struct BlockPalette {
    blocks: Box<[u16]>,
    block_palette: Vec<u32>,
}
impl BlockPalette {
    pub fn from_block_list(blocks: [[[u32; 16]; 16]; 16]) -> Self {
        let mut block_palette = Vec::new();
        for blocks in &blocks {
            for blocks in blocks {
                for block in blocks {
                    if !block_palette.contains(block) {
                        block_palette.push(*block);
                    }
                }
            }
        }
        Self { block_palette }
    }
    pub fn bits_per_block(&self) -> u8 {
        (self.block_palette.len() as f32).log2().ceil() as u8
    }
    fn internal_block_position(x: u8, y: u8, z: u8, pallete_size: u16) -> (usize, u8, u16) {
        let bits_per_block = (pallete_size as f32).log2().ceil() as u8;
        let x = x as u16;
        let y = y as u16;
        let z = z as u16;
        let bit_position = (x + (y * 16u16) + (z * 16u16 * 16u16)) as u32 * bits_per_block as u32;
        (
            (bit_position / 16) as usize,
            (bit_position % 16) as u8,
            2u16.pow(bits_per_block as u32) - 1,
        )
    }
}
*/
