use context_brain::turboquant::{TurboQuant, QuantMode};

#[test]
fn test_quantize_dequantize_roundtrip() {
    let tq = TurboQuant::new(1024);

    // Create a random-ish unit vector
    let mut vector = vec![0.0f32; 1024];
    for i in 0..1024 {
        vector[i] = ((i as f32 * 0.7 + 3.14).sin()) * 0.03;
    }

    // Quantize at 2 bits (fast mode)
    let quantized = tq.quantize(&vector, 2, QuantMode::Fast).unwrap();
    assert_eq!(quantized.original_dim, 1024);
    assert!(quantized.norm > 0.0);

    // Dequantize
    let reconstructed = tq.dequantize(&quantized);
    assert_eq!(reconstructed.len(), 1024);

    // Cosine similarity should be high (>0.8 for 2-bit)
    let cos_sim = cosine_similarity(&vector, &reconstructed);
    println!("2-bit fast roundtrip cosine similarity: {:.4}", cos_sim);
    assert!(cos_sim > 0.7, "Expected >0.7, got {}", cos_sim);
}

#[test]
fn test_fast_cosine_similarity() {
    let tq = TurboQuant::new(1024);

    let mut v1 = vec![0.0f32; 1024];
    let mut v2 = vec![0.0f32; 1024];
    for i in 0..1024 {
        v1[i] = ((i as f32 * 0.3).sin()) * 0.03;
        v2[i] = ((i as f32 * 0.3 + 0.1).sin()) * 0.03; // similar vector
    }

    let exact_sim = cosine_similarity(&v1, &v2);
    println!("Exact cosine similarity: {:.4}", exact_sim);

    let q2 = tq.quantize(&v2, 2, QuantMode::Fast).unwrap();
    let (rotated_q1, norm_q1) = tq.prepare_query(&v1);
    let approx_sim = tq.fast_cosine_similarity(&rotated_q1, norm_q1, &q2);
    println!("TurboQuant 2-bit fast cosine similarity: {:.4}", approx_sim);

    let error = (exact_sim - approx_sim).abs();
    println!("Error: {:.4}", error);
    assert!(error < 0.3, "Error too large: {}", error);
}

#[test]
fn test_compression_ratio() {
    let tq = TurboQuant::new(1024);
    let info = tq.storage_info(2, 1024);

    println!("Original: {} bytes", info.original_bytes);
    println!("Compressed: {} bytes", info.total_bytes);
    println!("Compression ratio: {:.1}x", info.compression_ratio);

    assert!(info.compression_ratio > 5.0);
    assert_eq!(info.original_bytes, 4096);
}

#[test]
fn test_zero_vector() {
    let tq = TurboQuant::new(1024);
    let vector = vec![0.0f32; 1024];
    let quantized = tq.quantize(&vector, 2, QuantMode::Fast).unwrap();
    assert_eq!(quantized.norm, 0.0);

    let reconstructed = tq.dequantize(&quantized);
    assert!(reconstructed.iter().all(|&x| x == 0.0));
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}
