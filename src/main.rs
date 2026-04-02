use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::time::Instant;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use clap::Parser;
use flate2::read::MultiGzDecoder;
use bzip2::read::BzDecoder;
use zstd::stream::read::Decoder as ZstdDecoder;
use xz2::read::XzDecoder;

enum CompressionType { Gzip, Bzip2, Zstd, Xz }

fn detect_compression_from_ext(path: &str) -> Option<CompressionType> {
    if path.ends_with(".gz") || path.ends_with(".gzip") { return Some(CompressionType::Gzip); }
    if path.ends_with(".bz2") { return Some(CompressionType::Bzip2); }
    if path.ends_with(".zst") || path.ends_with(".zstd") { return Some(CompressionType::Zstd); }
    if path.ends_with(".xz") || path.ends_with(".lzma") { return Some(CompressionType::Xz); }
    None
}

fn detect_compression_from_magic(buf: &[u8]) -> Option<CompressionType> {
    if buf.len() >= 2 && buf[0] == 0x1f && buf[1] == 0x8b { return Some(CompressionType::Gzip); }
    if buf.len() >= 3 && &buf[..3] == b"BZh" { return Some(CompressionType::Bzip2); }
    if buf.len() >= 4 && &buf[..4] == b"\x28\xb5\x2f\xfd" { return Some(CompressionType::Zstd); }
    if buf.len() >= 6 && &buf[..6] == b"\xfd7zXZ\x00" { return Some(CompressionType::Xz); }
    None
}

fn open_fasta_reader(path: &str) -> io::Result<Box<dyn BufRead>> {
    if path == "-" {
        // Use Stdin (not StdinLock) — Stdin: Read + Send + 'static, no lifetime issue
        let mut reader = BufReader::new(io::stdin());
        let peeked = reader.fill_buf()?;
        let comp = detect_compression_from_magic(peeked);
        return match comp {
            Some(CompressionType::Gzip)  => Ok(Box::new(BufReader::new(MultiGzDecoder::new(reader)))),
            Some(CompressionType::Bzip2) => Ok(Box::new(BufReader::new(BzDecoder::new(reader)))),
            Some(CompressionType::Zstd)  => Ok(Box::new(BufReader::new(ZstdDecoder::new(reader)?))),
            Some(CompressionType::Xz)    => Ok(Box::new(BufReader::new(XzDecoder::new(reader)))),
            None => Ok(Box::new(reader)),
        };
    }

    // [5] 단일 파일 오픈 + Seek: 기존의 magic byte 확인용 이중 오픈 제거
    let comp = detect_compression_from_ext(path);
    let mut file = File::open(path)?;

    let comp = comp.or_else(|| {
        let mut buf = [0u8; 6];
        file.read_exact(&mut buf).ok()?;
        file.seek(SeekFrom::Start(0)).ok()?;
        detect_compression_from_magic(&buf)
    });

    match comp {
        Some(CompressionType::Gzip)  => Ok(Box::new(BufReader::new(MultiGzDecoder::new(file)))),
        Some(CompressionType::Bzip2) => Ok(Box::new(BufReader::new(BzDecoder::new(file)))),
        Some(CompressionType::Zstd)  => Ok(Box::new(BufReader::new(ZstdDecoder::new(file)?))),
        Some(CompressionType::Xz)    => Ok(Box::new(BufReader::new(XzDecoder::new(file)))),
        None => Ok(Box::new(BufReader::new(file))),
    }
}

#[derive(Parser, Debug)]
#[command(author="Mojiri", version="1.0.0", about="BLAST 결과의 accession을 가지고 fasta 찾아서 모아줌(출력).", long_about = None)]
struct Args {
    /// Direct accession input: string or '-' for stdin (newline-separated)
    #[arg(conflicts_with = "blast_output")]
    input: Option<String>,

    /// Path to the BLAST output file
    #[arg(short, long)]
    blast_output: Option<String>,

    /// Subject field indices (comma-separated, e.g., 1,2,3)
    #[arg(short, long, value_delimiter = ',')]
    subject_fields: Vec<usize>,

    /// Path to the database FASTA file (optional - if not provided, uses online mode)
    #[arg(short, long)]
    db_fasta: Option<String>,

    /// Number of parallel jobs for online fetch (default: 4, only used when -d is not provided)
    #[arg(short, long, default_value = "4")]
    jobs: usize,

    /// Exclude mode: extract sequences NOT in BLAST results
    #[arg(short, long)]
    exclude: bool,

    /// Contain mode: match if header contains the search string
    #[arg(short, long, conflicts_with = "last")]
    contain: bool,

    /// Last mode: match if header ends with the search string
    #[arg(short, long, conflicts_with = "contain")]
    last: bool,
}

// NCBI efetch function to fetch FASTA from online (single attempt, no retry)
async fn fetch_fasta_from_ncbi(accession: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let url = format!(
        "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/efetch.fcgi?db=nucleotide&id={}&rettype=fasta&retmode=text",
        accession
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let response = client.get(&url).send().await?;

    if response.status().is_success() {
        let text = response.text().await?;
        if text.starts_with('>') {
            return Ok(text);
        } else {
            return Err(format!("Invalid FASTA format for {}", accession).into());
        }
    } else {
        return Err(format!("HTTP error: {}", response.status()).into());
    }
}

// Online mode: fetch sequences from NCBI
async fn fetch_sequences_online(
    uniq_subjects: HashSet<String>,
    args: &Args,
) -> io::Result<()> {
    let total = uniq_subjects.len();
    eprintln!("Online mode: fetching {} unique accessions from NCBI", total);
    eprintln!("Using {} parallel jobs", args.jobs);

    let accessions: Vec<String> = uniq_subjects.into_iter().collect();

    // [6] AtomicUsize: Mutex<usize> 대신 lock-free 카운터
    let success_count = Arc::new(AtomicUsize::new(0));

    // [6] std::sync::mpsc 채널 + 블로킹 스레드:
    //     StdoutLock은 Send가 아니므로 tokio::spawn 불가 → 별도 OS 스레드에서 소유
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let writer_handle = std::thread::spawn(move || {
        let stdout = io::stdout();
        let mut out = BufWriter::with_capacity(1 << 16, stdout.lock());
        for fasta in rx {
            let _ = out.write_all(fasta.as_bytes());
        }
        let _ = out.flush();
    });

    // Multiple rounds of retry
    let max_rounds = 10;
    // [6] clone 제거: accessions를 move로 초기화
    let mut remaining_accessions = accessions;

    for round in 1..=max_rounds {
        if remaining_accessions.is_empty() {
            eprintln!("All remaining accessions successfully downloaded!");
            break;
        }

        eprintln!("\n=== ROUND {} ===", round);
        if round <= 3 {
            eprintln!("Using parallel processing strategy");
        } else {
            eprintln!("Using sequential processing strategy for stubborn accessions");
        }
        let total_in_round = remaining_accessions.len();
        eprintln!("[Round {}] Processing {} accessions...", round, total_in_round);

        let round_failed = Arc::new(Mutex::new(Vec::new()));

        if round <= 3 {
            // Round 1-3: Parallel mode (quiet - no individual SUCCESS/FAILED messages)
            let semaphore = Arc::new(tokio::sync::Semaphore::new(args.jobs));
            let mut handles = vec![];

            for accession in remaining_accessions.iter() {
                let accession = accession.clone();
                let success_count = Arc::clone(&success_count);
                let round_failed = Arc::clone(&round_failed);
                let tx = tx.clone();
                let permit = semaphore.clone().acquire_owned().await.unwrap();

                let handle = tokio::spawn(async move {
                    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

                    match fetch_fasta_from_ncbi(&accession).await {
                        Ok(fasta) => {
                            let _ = tx.send(fasta);
                            success_count.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(_e) => {
                            round_failed.lock().unwrap().push(accession.clone());
                        }
                    }

                    drop(permit);
                });

                handles.push(handle);
            }

            // Wait for all tasks to complete
            for handle in handles {
                let _ = handle.await;
            }
        } else {
            // Round 4-10: Sequential mode with SUCCESS/FAILED messages
            for accession in remaining_accessions.iter() {
                eprintln!("  Trying {}...", accession);

                match fetch_fasta_from_ncbi(&accession).await {
                    Ok(fasta) => {
                        let _ = tx.send(fasta);
                        success_count.fetch_add(1, Ordering::Relaxed);

                        eprintln!("SUCCESS: {}", accession);
                        eprintln!("    ✓ Success");
                    }
                    Err(_e) => {
                        eprintln!("FAILED: {}", accession);
                        eprintln!("    ✗ Failed");
                        round_failed.lock().unwrap().push(accession.clone());
                    }
                }

                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
        }

        // [6] round_failed_list: clone 대신 drain으로 소유권 이동
        let round_failed_list = {
            let mut guard = round_failed.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        let round_success = remaining_accessions.len() - round_failed_list.len();
        eprintln!("[Round {}] Success: {}, Failed: {}", round, round_success, round_failed_list.len());

        remaining_accessions = round_failed_list;

        if !remaining_accessions.is_empty() && round < max_rounds {
            let wait_time = if round == 3 { 10 } else { 5 };
            eprintln!("Waiting {} seconds before retry round {}...", wait_time, round + 1);
            tokio::time::sleep(tokio::time::Duration::from_secs(wait_time)).await;
        }
    }

    let final_success = success_count.load(Ordering::Relaxed);
    let final_failed = remaining_accessions.len();

    // writer 스레드 종료: tx drop으로 채널 닫기 → rx 루프 종료 → 스레드 join
    drop(tx);
    let _ = writer_handle.join();

    eprintln!("\n=== FINAL SUMMARY ===");
    eprintln!("Total: {}", total);
    eprintln!("Success: {}", final_success);
    eprintln!("Failed: {}", final_failed);

    if final_failed > 0 {
        eprintln!("\nWARNING: {} accessions could not be downloaded after {} rounds:", final_failed, max_rounds);
        eprintln!("These may be invalid accessions or have persistent network issues:");
        for acc in remaining_accessions.iter().take(5) {
            eprintln!("  {}", acc);
        }
        if final_failed > 5 {
            eprintln!("  ... and {} more", final_failed - 5);
        }
    }

    Ok(())
}

// Local mode: search in local FASTA file
fn search_local_fasta(
    reader: Box<dyn BufRead>,
    uniq_subjects: &mut HashSet<String>,
    args: &Args,
) -> io::Result<()> {
    eprintln!("Reading the Database is started.");
    let start_time = Instant::now();

    // [1] BufWriter 도입: stdout 직접 쓰기 + 64KB 버퍼로 syscall 감소
    let stdout = io::stdout();
    let mut out = BufWriter::with_capacity(1 << 16, stdout.lock());
    let mut processing = false;
    let mut seq_count = 0usize;
    // [1] Vec<String> 제거: current_fasta 완전히 삭제
    let mut raw_buf = Vec::with_capacity(8192);
    let mut reader = reader;

    loop {
        raw_buf.clear();
        let n = reader.read_until(b'\n', &mut raw_buf)?;
        if n == 0 { break; }

        if raw_buf.first() == Some(&b'>') {
            // ── Header 라인 ──────────────────────────────────────────────────
            // trim trailing \r\n (header에서만 필요)
            let len = raw_buf.len();
            let trimmed_len = raw_buf[..len].iter().rev()
                .take_while(|&&b| b == b'\n' || b == b'\r')
                .count();
            raw_buf.truncate(len - trimmed_len);

            // UTF-8 변환은 header 라인에서만 수행 (서열 라인은 바이트 직접 처리)
            let line_cow = String::from_utf8_lossy(&raw_buf);
            let line: &str = &line_cow;

            // 이전 시퀀스가 끝날 때 seq_count 증가
            if processing {
                seq_count += 1;
                if seq_count % 10000 == 0 {
                    eprintln!("Now {} sequences is saved.", seq_count);
                }
            }

            // [3] accession 한 번만 추출 (매칭 체크 + remove() 모두 재사용)
            let accn_for_default = if !args.contain && !args.last {
                let raw = if let Some((a, _)) = line.split_once(' ') { a } else { line };
                if raw.len() > 1 { Some(&raw[1..]) } else { None }
            } else {
                None
            };

            // 매칭 모드에 따라 다른 로직 적용
            let found_match = if args.contain {
                uniq_subjects.iter().any(|pattern| line.contains(pattern.as_str()))
            } else if args.last {
                uniq_subjects.iter().any(|pattern| line.ends_with(pattern.as_str()))
            } else {
                accn_for_default.map_or(false, |a| uniq_subjects.contains(a))
            };

            // exclude 모드에 따라 processing 로직을 다르게 적용
            if args.exclude {
                processing = !found_match;
            } else {
                processing = found_match;
                if found_match {
                    if let Some(a) = accn_for_default {
                        uniq_subjects.remove(a);
                    }
                }
            }

            if processing {
                out.write_all(line.as_bytes())?;
                out.write_all(b"\n")?;
            }
        } else if processing {
            // ── 서열 라인 (출력 대상) ────────────────────────────────────────
            // UTF-8 변환 없이 바이트 직접 출력.
            // read_until은 '\n'을 raw_buf에 포함시키므로 그대로 write 가능.
            // Windows 줄바꿈(\r\n)만 처리.
            if raw_buf.ends_with(b"\r\n") {
                out.write_all(&raw_buf[..raw_buf.len() - 2])?;
                out.write_all(b"\n")?;
            } else {
                out.write_all(&raw_buf)?; // '\n' 포함
            }
        }
        // ── 서열 라인 (출력 불필요) ──────────────────────────────────────────
        // processing == false && !starts_with('>'):
        // trim도 UTF-8 변환도 하지 않고 즉시 다음 라인으로

        // early termination
        if !args.exclude && !processing && uniq_subjects.is_empty() {
            break;
        }
    }

    // 마지막 시퀀스가 processing 중이었다면 카운트
    if processing {
        seq_count += 1;
        if seq_count % 10000 == 0 {
            eprintln!("Now {} sequences is saved.", seq_count);
        }
    }

    out.flush()?;

    let end_time = Instant::now();
    eprintln!("Process the work: {}s", end_time.duration_since(start_time).as_secs_f64());

    Ok(())
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = Args::parse();

    // Validation: at least one of blast_output or input must be provided
    if args.blast_output.is_none() && args.input.is_none() {
        eprintln!("Error: Either --blast-output (-b) or positional input must be provided");
        std::process::exit(1);
    }

    if args.exclude && args.db_fasta.is_none() {
        eprintln!("Error: --exclude mode requires --db-fasta to be specified");
        std::process::exit(1);
    }

    if args.jobs < 1 || args.jobs > 20 {
        eprintln!("Warning: --jobs should be between 1 and 20 (using {}).", args.jobs);
    }

    if args.db_fasta.is_none() && (args.contain || args.last) {
        eprintln!("Warning: --contain and --last options are only used in local mode (with --db-fasta)");
    }

    if args.db_fasta.as_deref() == Some("-") && args.input.as_deref() == Some("-") {
        eprintln!("Error: cannot use stdin ('-') for both accession input and -d simultaneously");
        std::process::exit(1);
    }

    let mut uniq_subjects = HashSet::new();
    let start_time = Instant::now();

    if let Some(input) = &args.input {
        // Positional input mode: stdin if "-", otherwise treat as direct string
        if input == "-" {
            eprintln!("Reading accessions from stdin.");
            let stdin = io::stdin();
            for line in stdin.lock().lines() {
                let line = line?;
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    uniq_subjects.insert(trimmed.to_string());
                }
            }
        } else {
            eprintln!("Reading accessions from string.");
            for line in input.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    uniq_subjects.insert(trimmed.to_string());
                }
            }
        }
    } else if let Some(blast_path) = &args.blast_output {
        eprintln!("Reading the BLAST output is started.");

        let blast_output_file = File::open(blast_path)?;
        let blast_output_file = BufReader::new(blast_output_file);

        // [4] fields_to_use를 루프 밖에서 한 번만 계산
        let fields_to_use: Vec<usize> = if args.subject_fields.is_empty() {
            vec![1]
        } else {
            args.subject_fields.clone()
        };
        let max_field = fields_to_use.iter().copied().max().unwrap_or(1);

        for line in blast_output_file.lines() {
            let line = line?;
            // [4] collect::<Vec<_>>() 제거: 인덱스 기반 iterator 탐색
            let mut col_idx = 1usize;
            for col in line.split('\t') {
                if fields_to_use.contains(&col_idx) {
                    let trimmed = col.trim();
                    if !trimmed.is_empty() {
                        uniq_subjects.insert(trimmed.to_string());
                    }
                }
                col_idx += 1;
                if col_idx > max_field { break; }
            }
        }
    }

    let end_time = Instant::now();
    eprintln!("Reading input: {}s", end_time.duration_since(start_time).as_secs_f64());

    if args.exclude {
        eprintln!("Exclude mode: extracting sequences NOT in BLAST results");
    } else {
        eprintln!("Include mode: extracting sequences in BLAST results");
    }

    // Choose mode based on whether db_fasta is provided
    if let Some(db_fasta_path) = &args.db_fasta {
        // Local mode
        eprintln!("Local mode: searching in database file");
        let reader = open_fasta_reader(db_fasta_path)?;
        search_local_fasta(reader, &mut uniq_subjects, &args)?;
    } else {
        // Online mode
        fetch_sequences_online(uniq_subjects, &args).await?;
    }

    Ok(())
}
