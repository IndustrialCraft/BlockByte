use block_byte_client::run;

fn main() {
    pollster::block_on(run());
}
