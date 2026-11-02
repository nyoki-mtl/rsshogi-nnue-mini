use rsnn_package::{
    CONTAINER_VERSION, DType, Digest, Error, EvaluationDescriptor, EvaluatorExpectation,
    ExportSpec, HEADER_LEN, Manifest, Package, PackageReader, PackageWriter, PaddingPolicy,
    Provenance, Rational, TensorDescriptor, TensorRole, canonical_json, evaluation_spec_hash,
    export_spec_hash, sha256, verify_sfnnv15_threat_schema,
};
use std::fmt::Write as _;

fn sample_package() -> Package {
    let payload = vec![1, 2, 3, 4, 0, 0];
    let tensor = TensorDescriptor {
        name: "output_bias".into(),
        role: TensorRole::OutputBias,
        dtype: DType::I16,
        shape: vec![2],
        scale: Rational { numerator: 1, denominator: 128 },
        offset: 0,
        length: 6,
        digest: sha256(&payload),
        padding: 2,
    };
    let mut descriptor = EvaluationDescriptor::sfnnv15_v2(vec![tensor.clone()]);
    descriptor.recipe_id = "fixture_v1".into();
    descriptor.recipe_version = 1;
    descriptor.feature_schema.id = "fixture_shogi".into();
    descriptor.feature_schema.version = 1;
    let base = descriptor
        .feature_schema
        .blocks
        .iter_mut()
        .find(|block| block.name == "base")
        .expect("fixture base block");
    base.feature_count = 73_305;
    base.indexing = "halfka_hm_v1".into();
    let provenance = Provenance {
        training_spec_hash: sha256(b"training"),
        export_spec_hash: sha256(b"export"),
        source_plan_hash: sha256(b"plan"),
        source_checkpoint_digest: sha256(b"checkpoint"),
        source_dataset_identity: "sha256:fixture-dataset".into(),
    };
    let manifest =
        Manifest::new(descriptor, provenance, vec![tensor], Vec::new(), sha256(&payload)).unwrap();
    Package { manifest, payload }
}

fn raw_package(manifest: &Manifest, payload: &[u8]) -> Vec<u8> {
    let manifest = canonical_json(manifest).unwrap();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RSNNPKG\0");
    bytes.extend_from_slice(&CONTAINER_VERSION.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&(manifest.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&manifest);
    bytes.extend_from_slice(payload);
    bytes
}

fn rewrite_manifest(package: &Package, mutate: impl FnOnce(&mut Manifest)) -> Vec<u8> {
    let mut manifest = package.manifest.clone();
    mutate(&mut manifest);
    raw_package(&manifest, &package.payload)
}

#[test]
fn descriptor_and_export_hashes_are_golden() {
    let package = sample_package();
    assert_eq!(
        evaluation_spec_hash(&package.manifest.evaluation_descriptor).unwrap().to_hex(),
        "5f37054c5324d84fef8a81417d1c352cfe84be6575b4955747987e7ad63895d5"
    );
    let export = ExportSpec {
        schema_version: 1,
        artifact_projection_version: 1,
        rounding: "round_away_from_zero".into(),
        tensor_order: vec!["output_bias".into()],
        padding: PaddingPolicy::ZeroFill,
        container_version: CONTAINER_VERSION,
        auxiliary_artifacts: Vec::new(),
    };
    assert_eq!(
        export_spec_hash(&export).unwrap().to_hex(),
        "3dc1ade9f5dd5f3ff3bac1aba5ecbde24b319ede87319c46fa597c15a7bdaae5"
    );
}

#[test]
fn runtime_descriptor_fixtures_are_golden() {
    let sfnnv15_v2 = EvaluationDescriptor::sfnnv15_v2_schema();
    verify_sfnnv15_threat_schema(
        &sfnnv15_v2,
        Digest::from_hex("7dbe1308bf7b7a4a535dad79ad29f2ec8e2bc69811fbe4b4d54cd859f35888fd")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        verify_sfnnv15_threat_schema(&sfnnv15_v2, Digest::ZERO),
        Err(Error::DescriptorMismatch { field: "threat_schema_digest" })
    );
    assert_eq!(sfnnv15_v2.recipe_id, "sfnnv15_v2");
    assert_eq!(sfnnv15_v2.recipe_version, 2);
    assert_eq!(sfnnv15_v2.feature_schema.version, 2);
    let base = sfnnv15_v2.feature_schema.blocks.iter().find(|block| block.name == "base").unwrap();
    assert_eq!(base.feature_count, 72_675);
    assert_eq!(base.indexing, "halfka_hm_direct_v2");
    assert_eq!(sfnnv15_v2.tensors[0].shape, [72_675, 1_024]);
    assert_eq!(sfnnv15_v2.tensors[3].shape, [72_675, 8]);
    assert_eq!(sfnnv15_v2.tensors[0].length, 72_675 * 1_024 * 2);
    assert_eq!(sfnnv15_v2.tensors[1].offset, 72_675 * 1_024 * 2);
    assert_eq!(sfnnv15_v2.tensors[3].length, 72_675 * 8 * 4);
    assert_eq!(
        sfnnv15_v2.tensors[4].offset,
        sfnnv15_v2.tensors[3].offset + sfnnv15_v2.tensors[3].length
    );
    assert_eq!(
        sfnnv15_v2.hash().unwrap().to_hex(),
        "876178dc142ca2aa0dd89a7b732066c2d93f09a5eaf39fd54cc350b8387294ae"
    );
}

#[test]
fn canonical_manifest_package_bytes_and_digest_are_golden() {
    let package = sample_package();
    let manifest = String::from_utf8(package.manifest.canonical_json().unwrap()).unwrap();
    assert_eq!(manifest, include_str!("fixtures/canonical-manifest-v1.json").trim_end());

    let bytes = PackageWriter::write(&package).unwrap();
    let header_hex = bytes[..HEADER_LEN].iter().fold(String::new(), |mut output, byte| {
        write!(output, "{byte:02x}").unwrap();
        output
    });
    assert_eq!(header_hex, "52534e4e504b4700010000003b080000000000000600000000000000");
    assert_eq!(
        sha256(&bytes).to_hex(),
        "258b69837ed860d7bf20b86c72683263b8c0dffbde97cd3cb685b3f1cd1f0917"
    );
}

#[test]
fn writer_is_deterministic_and_reader_round_trips() {
    let package = sample_package();
    let first = PackageWriter::write(&package).unwrap();
    let second = PackageWriter::write(&package).unwrap();
    assert_eq!(first, second);

    let digest = sha256(&first);
    let read = PackageReader::default().read(&first, Some(digest)).unwrap();
    assert_eq!(read.manifest, package.manifest);
    assert_eq!(read.payload, package.payload);
    assert_eq!(read.package_digest, digest);
    let tensor = read.tensor("output_bias").unwrap();
    assert_eq!(tensor.data, &[1, 2, 3, 4]);
    assert_eq!(tensor.padding, &[0, 0]);
    assert_eq!(tensor.descriptor.name, "output_bias");
    assert_eq!(read.tensor("missing"), Err(Error::MissingEntry("missing".into())));
}
#[test]
fn package_evaluation_hash_and_evaluator_expectation_are_verified() {
    let package = sample_package();
    let bytes = PackageWriter::write(&package).unwrap();
    assert_eq!(
        PackageReader::default().read(&bytes, Some(Digest::ZERO)),
        Err(Error::PackageDigestMismatch)
    );

    let bad_hash = rewrite_manifest(&package, |manifest| {
        manifest.evaluation_spec_hash = Digest::ZERO;
    });
    assert_eq!(
        PackageReader::default().read(&bad_hash, None),
        Err(Error::EvaluationSpecHashMismatch)
    );

    let expectation =
        EvaluatorExpectation::new(package.manifest.evaluation_descriptor.clone()).unwrap();
    expectation.verify(&package.manifest.evaluation_descriptor).unwrap();
    let mut incompatible = package.manifest.evaluation_descriptor;
    incompatible.integer_evaluation.output_multiplier += 1;
    assert_eq!(
        expectation.verify(&incompatible),
        Err(Error::DescriptorMismatch { field: "integer_evaluation" })
    );
}
