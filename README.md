# accn_to_fasta

BLAST 결과의 accession을 기반으로 FASTA 파일에서 해당 sequence를 추출합니다.

`seqkit grep`, `seqtk subseq`과 유사한 기능을 제공하며, 처리 속도가 더 빠릅니다.

> **Benchmark** — genome sequence FASTA (sequence 1,404,310개 / 363 GB / 평균 길이 272,225.3 bp / 최소 84 bp / 최대 1,007,622,168 bp) 에서 random accession 100개를 query로 사용한 결과입니다.
>
> _추후 benchmark 추가 예정_

---

## Usage

```
accn_to_fasta [OPTIONS] [INPUT]
```

### Arguments

| Argument | Description |
|----------|-------------|
| `[INPUT]` | Accession 직접 입력: 문자열 또는 `-` (stdin, 줄바꿈으로 구분) |

### Options

| Option | Description |
|--------|-------------|
| `-b, --blast-output <BLAST_OUTPUT>` | BLAST output 파일 경로 (TSV 형식) |
| `-s, --subject-fields <SUBJECT_FIELDS>` | Subject field 인덱스 (쉼표 구분, 예: `1,2,3`) |
| `-d, --db-fasta <DB_FASTA>` | 데이터베이스 FASTA 파일 경로 (미지정 시 online 모드 사용) |
| `-j, --jobs <JOBS>` | Online fetch 시 병렬 작업 수 (기본값: `4`, `-d` 미사용 시에만 적용) |
| `-e, --exclude` | Exclude 모드: BLAST 결과에 **없는** sequence를 추출 |
| `-c, --contain` | Contain 모드: header에 검색 문자열이 **포함**된 경우 매칭 |
| `-l, --last` | Last 모드: header가 검색 문자열로 **끝나는** 경우 매칭 |
| `-h, --help` | 도움말 출력 |
| `-V, --version` | 버전 출력 |

---

## Examples

아래 예시는 `test.fna`를 기준으로 작성되었습니다.  
추출된 sequence는 `>` 리다이렉션을 통해 파일로 저장할 수 있습니다.

### 1. Accession으로 검색

**단일 accession 조회**

```bash
$ accn_to_fasta Accn1 -d test.fna
>Accn1 Gangdaniel Dalmeun Imo
NAANEUNSARAMGANGDANIELDALMEUNIMOGADASIBOGEDOENEUNGEDASIGEUTT
AECHEOREOMANDAMGEEOMMABOMYEONNEUKKYEOJINEUNGEOLSUDOITNEUNGEO
IMEOMMADO
```

**단일 accession — exclude 모드 (`-e`)**

```bash
$ accn_to_fasta Accn1 -d test.fna -e
>Meme5 Eoje Nae Sesangi Muneojyeosseo
EOJENAESESANGIMUNEOJYEOSSEOGOBAEKBADATNEUNDENAEGAGEOJEOLHAES
...
>Jakpum Seolreongtang Wae Mot Meogeum
SEOLREONGTANGEULSADANOATNEUNDEWAEMEOKJIREULMOTHANIWAEMEOKJIR
...
```

**파일 또는 stdin으로 accession 목록 전달**

```bash
$ cat accn.list
Jakpum
Accn1

# 파일로 전달
$ accn_to_fasta -b accn_list -d test.fna

# stdin으로 전달
$ cat accn_list | accn_to_fasta - -d test.fna
```

```
>Accn1 Gangdaniel Dalmeun Imo
NAANEUNSARAMGANGDANIELDALMEUNIMOGADASIBOGEDOENEUNGEDASIGEUTT
AECHEOREOMANDAMGEEOMMABOMYEONNEUKKYEOJINEUNGEOLSUDOITNEUNGEO
IMEOMMADO
>Jakpum Seolreongtang Wae Mot Meogeum
SEOLREONGTANGEULSADANOATNEUNDEWAEMEOKJIREULMOTHANIWAEMEOKJIR
EULMOTHANIGOESANGHAGEDOONEUREUNUNSUGAJOTEONIMAN
```

**파일 목록 — exclude 모드 (`-e`)**

```bash
$ accn_to_fasta -b accn_list -d test.fna -e
>Meme5 Eoje Nae Sesangi Muneojyeosseo
EOJENAESESANGIMUNEOJYEOSSEOGOBAEKBADATNEUNDENAEGAGEOJEOLHAES
...
```

---

### 2. Header 전체로 검색

#### Contain 모드 (`-c`) — header에 문자열이 포함된 경우 매칭

```bash
$ accn_to_fasta eo -d test.fna -c
>Meme5 Eoje Nae Sesangi Muneojyeosseo
EOJENAESESANGIMUNEOJYEOSSEOGOBAEKBADATNEUNDENAEGAGEOJEOLHAES
...
>Jakpum Seolreongtang Wae Mot Meogeum
SEOLREONGTANGEULSADANOATNEUNDEWAEMEOKJIREULMOTHANIWAEMEOKJIR
...

$ accn_to_fasta eo -d test.fna -ce   # contain + exclude
>Accn1 Gangdaniel Dalmeun Imo
NAANEUNSARAMGANGDANIELDALMEUNIMOGADASIBOGEDOENEUNGEDASIGEUTT
...
```

#### Last 모드 (`-l`) — header가 문자열로 끝나는 경우 매칭

```bash
$ accn_to_fasta o -d test.fna -l
>Accn1 Gangdaniel Dalmeun Imo
...
>Meme5 Eoje Nae Sesangi Muneojyeosseo
...

$ accn_to_fasta o -d test.fna -le   # last + exclude
>Jakpum Seolreongtang Wae Mot Meogeum
...
```

#### Header 목록 파일과 contain 모드 조합

```bash
$ cat header.list
>Meme5 Eoje Nae Sesangi Muneojyeosseo
>Jakpum Seolreongtang Wae Mot Meogeum

$ accn_to_fasta -b header.list -d test.fna -c
>Meme5 Eoje Nae Sesangi Muneojyeosseo
EOJENAESESANGIMUNEOJYEOSSEOGOBAEKBADATNEUNDENAEGAGEOJEOLHAES
...
>Jakpum Seolreongtang Wae Mot Meogeum
SEOLREONGTANGEULSADANOATNEUNDEWAEMEOKJIREULMOTHANIWAEMEOKJIR
...
```
