import re
import subprocess
import sys
import os


try:
    from korean_romanizer.romanizer import Romanizer
except ImportError:
    subprocess.check_call([sys.executable, '-m', 'pip', 'install', 'korean-romanizer'])
    from korean_romanizer.romanizer import Romanizer


def romanize_korean(text):
    return Romanizer(text).romanize()


def process_fna(input_path):
    with open(input_path, 'r', encoding='utf-8') as f:
        lines = f.readlines()

    fasta_blocks = []   # (header, sequence) pairs
    current_header = None
    current_seq = []

    for line in lines:
        stripped = line.rstrip('\n')
        if not stripped:
            continue

        if stripped.startswith('>'):
            if current_header is not None:
                fasta_blocks.append((current_header, ''.join(current_seq)))
            romanized = romanize_korean(stripped[1:])  # '>' 제외 로마자 변환
            title_cased = romanized.title()
            current_header = '>' + title_cased
            current_seq = []
        else:
            romanized = romanize_korean(stripped)
            letters_only = re.sub(r'[^a-zA-Z]', '', romanized).upper()
            current_seq.append(letters_only)

    if current_header is not None:
        fasta_blocks.append((current_header, ''.join(current_seq)))

    for header, seq in fasta_blocks:
        print(header)
        for i in range(0, len(seq), 60):
            print(seq[i:i+60])


if __name__ == '__main__':
    if len(sys.argv) != 2:
        print(f"Usage: python {os.path.basename(sys.argv[0])} <input.fna>", file=sys.stderr)
        sys.exit(1)
    process_fna(sys.argv[1])
