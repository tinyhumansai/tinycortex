# Run Your Own Benchmarks

The benchmark suite is included in the TinyCortex repository. You can run all benchmarks locally or target specific methods.

## Setup

```bash
pip install -r requirements.txt
bash scripts/download_corpus.sh
```

## Run Benchmarks

```bash
# Run all benchmarks
python run.py

# Run specific methods
python run.py --methods tinycortex,vdb --max-questions 10
```

## View Results

```bash
python scripts/chart.py --chart bar
```

## Configuration

Benchmarks are configured via `config.json` in the repository root:

| Parameter | Default | Description |
| --- | --- | --- |
| `corpus` | `sherlock_holmes` | Corpus to use for evaluation |
| `methods` | all | Methods to benchmark |
| `max_questions` | `0` (unlimited) | Limit number of questions per benchmark |
| `top_k` | `8` | Number of chunks to retrieve |
| `chunk_size` | `1200` | Token size per chunk |
| `chunk_overlap` | `200` | Overlap between chunks |
| `openai_model` | `gpt-4o-mini` | LLM used by methods |
| `embedding_model` | `text-embedding-3-small` | Embedding model |
| `ragas_judge_model` | `gpt-4o` | Judge model for RAGAS evaluation |
