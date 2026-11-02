use std::path::Path;

use rsnn_package::{
    CONTAINER_VERSION, Error, HEADER_LEN, Package, PackageReader, PackageWriter, canonical_json,
    sha256,
};

fn raw_package(package: &Package) -> Vec<u8> {
    let manifest = canonical_json(&package.manifest).unwrap();
    let mut bytes = Vec::with_capacity(HEADER_LEN + manifest.len() + package.payload.len());
    bytes.extend_from_slice(b"RSNNPKG\0");
    bytes.extend_from_slice(&CONTAINER_VERSION.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&(manifest.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&(package.payload.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&manifest);
    bytes.extend_from_slice(&package.payload);
    bytes
}

#[test]
#[ignore = "full 274 MB package qualification; run explicitly for release evidence"]
fn default_package_is_certified_and_writer_reader_reject_the_same_unsafe_bias() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/nnue/default.rsnn");
    let read = PackageReader::default().read_file(path, None).unwrap();
    let report = read.numeric_range_report.expect("exact SFNNv15 certificate");
    assert_eq!(report.accumulator_min, -25_744);
    assert_eq!(report.accumulator_max, 26_012);
    assert_eq!(report.psqt_sum_min, -998_339);
    assert_eq!(report.psqt_sum_max, 1_000_368);
    assert_eq!(report.psqt_difference_span_max, 1_973_941);

    let mut package = Package { manifest: read.manifest, payload: read.payload };
    let bias = package.manifest.tensors.iter().position(|tensor| tensor.name == "ft_bias").unwrap();
    let offset = usize::try_from(package.manifest.tensors[bias].offset).unwrap();
    package.payload[offset..offset + 2].copy_from_slice(&i16::MAX.to_le_bytes());
    let start = offset;
    let end = start + usize::try_from(package.manifest.tensors[bias].length).unwrap();
    let digest = sha256(&package.payload[start..end]);
    package.manifest.tensors[bias].digest = digest;
    package.manifest.evaluation_descriptor.tensors[bias].digest = digest;
    package.manifest.payload_digest = sha256(&package.payload);

    let writer_error = PackageWriter::write(&package).unwrap_err();
    assert!(matches!(writer_error, Error::NumericRangeViolation { .. }));
    let bytes = raw_package(&package);
    let reader_error = PackageReader::default().read(&bytes, None).unwrap_err();
    assert_eq!(reader_error, writer_error);
}
