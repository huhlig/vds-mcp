# Embedding Models for VDS

This directory contains ONNX embedding models that are compiled into the VDS binary for zero-configuration semantic search.

## Pre-Downloaded Model: all-MiniLM-L6-v2

The following files are included in this repository:

- `all-MiniLM-L6-v2.onnx` - The ONNX model file (~86MB)
- `tokenizer.json` - The HuggingFace tokenizer configuration (~455KB)

These files are embedded directly into the VDS binary at compile time on **Linux and macOS**.

## Platform Support

### Linux & macOS (Default)

Semantic search with embedded models is **enabled by default**:

```bash
cargo build --release
# Binary will be ~90MB larger but requires no external files
```

### Windows

**Note**: The `hnsw_vector_search` crate (used for ONNX inference) does not currently build on Windows. 

To build VDS on Windows **without** semantic search:

```bash
cargo build --release --no-default-features
```

Or to attempt building with semantic search (requires external model files):

```bash
# Place model files in a accessible location
cargo build --release --features semantic-search
# Then set environment variables:
# set VDS_EMBEDDING_MODEL_PATH=C:\path\to\all-MiniLM-L6-v2.onnx
# set VDS_EMBEDDING_TOKENIZER_PATH=C:\path\to\tokenizer.json
```

## Using External Model Files (All Platforms)

If you prefer **not** to embed the model in the binary (saves ~90MB), you can use external files:

```bash
# Build without embedded model
cargo build --release --features semantic-search --no-default-features

# Then provide model location via:
# 1. Environment variables
export VDS_EMBEDDING_MODEL_PATH=/path/to/all-MiniLM-L6-v2.onnx
export VDS_EMBEDDING_TOKENIZER_PATH=/path/to/tokenizer.json

# 2. Default location
mkdir -p ~/.vds/models
cp all-MiniLM-L6-v2.onnx ~/.vds/models/
cp tokenizer.json ~/.vds/models/

# 3. CLI flags
vds-mcp serve --model-path /path/to/model.onnx --tokenizer-path /path/to/tokenizer.json
```

## Downloading Different Models

If you want to use a different embedding model from HuggingFace:

```bash
# Using Python with transformers and optimum
pip install transformers optimum onnx onnxruntime

# Export the model to ONNX format
python -c "
from optimum.onnxruntime import ORTModelForFeatureExtraction
from transformers import AutoTokenizer

model_id = 'sentence-transformers/all-MiniLM-L6-v2'
model = ORTModelForFeatureExtraction.from_pretrained(model_id, export=True)
tokenizer = AutoTokenizer.from_pretrained(model_id)

model.save_pretrained('./models')
tokenizer.save_pretrained('./models')
"

# Or download pre-converted ONNX files from HuggingFace model hub
# https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2
```

### Building with Embedded Model

Once the model files are in place, build VDS with the embedding flag:

```bash
# Set environment variable to enable embedding
export VDS_EMBED_MODEL=1

# Build with semantic-search feature
cargo build --release --features semantic-search
```

### Runtime Fallback

If the model files are not present at build time, VDS will fall back to:

1. Environment variables: `VDS_EMBEDDING_MODEL_PATH` and `VDS_EMBEDDING_TOKENIZER_PATH`
2. Default location: `~/.vds/models/all-MiniLM-L6-v2.onnx` and `~/.vds/models/tokenizer.json`
3. CLI flags: `--model-path` and `--tokenizer-path`

## Model Details

- **Model**: all-MiniLM-L6-v2
- **Dimensions**: 384
- **License**: Apache 2.0
- **Size**: ~23MB (ONNX) + ~500KB (tokenizer)
- **Performance**: Fast inference, suitable for real-time embedding generation
