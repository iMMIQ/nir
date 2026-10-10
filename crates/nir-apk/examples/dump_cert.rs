fn main() {
    let key = nir_apk::key::SigningIdentity::from_seed(&[0x42u8; 32]).unwrap();
    std::fs::write(
        std::env::temp_dir().join("nir-cert.der"),
        key.certificate_der(),
    )
    .unwrap();
}
