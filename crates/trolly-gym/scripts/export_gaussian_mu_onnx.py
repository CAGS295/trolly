#!/usr/bin/env python3
"""Export a static [1, V×5] → [1,1] Gaussian μ ONNX graph (WP-039).

Offline stand-in (no torch / no weekday weights):

    python3 crates/trolly-gym/scripts/export_gaussian_mu_onnx.py \\
        --output checkpoints/microstructure/gaussian_mlp/mu.onnx \\
        --mean 0.8

Weekday `microstructure/gaussian_mlp` checkpoint (needs torch; does not train):

    python3 crates/trolly-gym/scripts/export_gaussian_mu_onnx.py \\
        --checkpoint-dir checkpoints/gpu_train_orchestrator/microstructure/gaussian_mlp \\
        --output checkpoints/microstructure/gaussian_mlp/mu.onnx

The stand-in is Gemm(observation, 0, mean) so `OnnxGaussianMeanPolicy` /
`ONNX_GAUSSIAN_MODEL_PATH` can load it without `--features gym-torch`.
3-logit `ONNX_MODEL_PATH` graphs are unchanged. Refuses
`_retired_unit_lot_microstructure`.
"""

from __future__ import annotations

import argparse
import json
import struct
import sys
from pathlib import Path

RETIRED = "_retired_unit_lot_microstructure"
DEFAULT_OBS_DIM = 40
PRODUCER = "trolly-gym"
IR_VERSION = 8
OPSET = 17
FLOAT = 1
INPUT_NAME = "observation"
OUTPUT_NAME = "mean"


def path_is_retired(path: Path) -> bool:
    return any(part.lower() == RETIRED for part in path.parts)


def put_varint(buf: bytearray, value: int) -> None:
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            buf.append(byte | 0x80)
        else:
            buf.append(byte)
            return


def put_tag(buf: bytearray, field: int, wire: int) -> None:
    put_varint(buf, (field << 3) | wire)


def put_int(buf: bytearray, field: int, value: int) -> None:
    put_tag(buf, field, 0)
    put_varint(buf, value)


def put_bytes(buf: bytearray, field: int, value: bytes) -> None:
    put_tag(buf, field, 2)
    put_varint(buf, len(value))
    buf.extend(value)


def put_string(buf: bytearray, field: int, value: str) -> None:
    put_bytes(buf, field, value.encode("utf-8"))


def put_msg(buf: bytearray, field: int, value: bytes) -> None:
    put_bytes(buf, field, value)


def encode_tensor(name: str, dims: list[int], values: list[float]) -> bytes:
    buf = bytearray()
    for dim in dims:
        put_int(buf, 1, dim)
    put_int(buf, 2, FLOAT)
    put_string(buf, 8, name)
    raw = b"".join(struct.pack("<f", v) for v in values)
    put_bytes(buf, 9, raw)
    return bytes(buf)


def encode_value_info(name: str, dims: list[int]) -> bytes:
    shape = bytearray()
    for dim in dims:
        dimension = bytearray()
        put_int(dimension, 1, dim)
        put_msg(shape, 1, bytes(dimension))
    tensor = bytearray()
    put_int(tensor, 1, FLOAT)
    put_msg(tensor, 2, bytes(shape))
    ty = bytearray()
    put_msg(ty, 1, bytes(tensor))
    info = bytearray()
    put_string(info, 1, name)
    put_msg(info, 2, bytes(ty))
    return bytes(info)


def encode_node(name: str, op_type: str, inputs: list[str], outputs: list[str]) -> bytes:
    buf = bytearray()
    for item in inputs:
        put_string(buf, 1, item)
    for item in outputs:
        put_string(buf, 2, item)
    put_string(buf, 3, name)
    put_string(buf, 4, op_type)
    return bytes(buf)


def write_recorded_mean_onnx(path: Path, obs_dim: int, mean: float) -> None:
    weights = [0.0] * obs_dim
    w = encode_tensor("W", [obs_dim, 1], weights)
    b = encode_tensor("B", [1], [mean])
    node = encode_node("gemm_mu", "Gemm", [INPUT_NAME, "W", "B"], [OUTPUT_NAME])
    graph = bytearray()
    put_msg(graph, 1, node)
    put_string(graph, 2, "gaussian_mu")
    put_msg(graph, 5, w)
    put_msg(graph, 5, b)
    put_msg(graph, 11, encode_value_info(INPUT_NAME, [1, obs_dim]))
    put_msg(graph, 12, encode_value_info(OUTPUT_NAME, [1, 1]))
    opset = bytearray()
    put_string(opset, 1, "")
    put_int(opset, 2, OPSET)
    model = bytearray()
    put_int(model, 1, IR_VERSION)
    put_string(model, 2, PRODUCER)
    put_msg(model, 7, bytes(graph))
    put_msg(model, 8, bytes(opset))
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(bytes(model))


def load_safetensors(path: Path) -> dict[str, tuple[tuple[int, ...], bytes]]:
    data = path.read_bytes()
    if len(data) < 8:
        raise SystemExit(f"truncated safetensors file: {path}")
    (header_len,) = struct.unpack_from("<Q", data, 0)
    start = 8
    end = start + header_len
    header = json.loads(data[start:end].decode("utf-8"))
    tensors: dict[str, tuple[tuple[int, ...], bytes]] = {}
    for name, meta in header.items():
        if name == "__metadata__":
            continue
        if not isinstance(meta, dict):
            continue
        dtype = meta.get("dtype")
        shape = tuple(int(d) for d in meta.get("shape", []))
        offsets = meta.get("data_offsets")
        if dtype != "F32" or not offsets:
            continue
        lo, hi = int(offsets[0]), int(offsets[1])
        tensors[name] = (shape, data[end + lo : end + hi])
    return tensors


def export_checkpoint_mu(checkpoint_dir: Path, output: Path, obs_dim: int) -> None:
    ckpt = checkpoint_dir / "latest.safetensors"
    if not ckpt.is_file():
        raise SystemExit(f"missing {ckpt}; pass --mean for a recorded stand-in")
    try:
        import torch
        import torch.nn as nn
    except ImportError as err:
        raise SystemExit(
            f"torch is required to export weekday weights ({err}); "
            "re-run with --mean <float> for the recorded stand-in"
        ) from err

    tensors = load_safetensors(ckpt)
    hidden: list[int] = []
    idx = 0
    while f"gauss_shared_{idx}.weight" in tensors:
        shape, _ = tensors[f"gauss_shared_{idx}.weight"]
        if len(shape) != 2:
            raise SystemExit(f"unexpected gauss_shared_{idx}.weight shape {shape}")
        hidden.append(int(shape[0]))
        idx += 1
    if not hidden:
        raise SystemExit(
            f"{ckpt} has no gauss_shared_*.weight tensors; "
            "is this a gaussian_mlp VarStore?"
        )

    class MuHead(nn.Module):
        def __init__(self) -> None:
            super().__init__()
            dims = [obs_dim, *hidden]
            self.shared = nn.ModuleList(
                nn.Linear(dims[i], dims[i + 1]) for i in range(len(hidden))
            )
            self.mu = nn.Linear(hidden[-1], 1)

        def forward(self, x: torch.Tensor) -> torch.Tensor:
            for layer in self.shared:
                x = torch.tanh(layer(x))
            return torch.tanh(self.mu(x))

    def copy_linear(layer: nn.Linear, prefix: str) -> None:
        weight = tensors.get(f"{prefix}.weight")
        bias = tensors.get(f"{prefix}.bias")
        if weight is None or bias is None:
            raise SystemExit(f"{ckpt} missing {prefix}.weight/bias")
        w_shape, w_bytes = weight
        b_shape, b_bytes = bias
        layer.weight.data.copy_(
            torch.frombuffer(bytearray(w_bytes), dtype=torch.float32).reshape(w_shape)
        )
        layer.bias.data.copy_(
            torch.frombuffer(bytearray(b_bytes), dtype=torch.float32).reshape(b_shape)
        )

    model = MuHead()
    for i, layer in enumerate(model.shared):
        copy_linear(layer, f"gauss_shared_{i}")
    copy_linear(model.mu, "gauss_mu")
    model.eval()
    dummy = torch.zeros(1, obs_dim, dtype=torch.float32)
    output.parent.mkdir(parents=True, exist_ok=True)
    torch.onnx.export(
        model,
        dummy,
        str(output),
        input_names=[INPUT_NAME],
        output_names=[OUTPUT_NAME],
        dynamic_axes=None,
        opset_version=OPSET,
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output",
        required=True,
        type=Path,
        help="Destination .onnx path (e.g. checkpoints/microstructure/gaussian_mlp/mu.onnx)",
    )
    parser.add_argument(
        "--obs-dim",
        type=int,
        default=DEFAULT_OBS_DIM,
        help="Flattened ladder size V×5 (default 40)",
    )
    parser.add_argument(
        "--mean",
        type=float,
        help="Recorded-mean stand-in bias (Gemm zeros + this μ). No torch required.",
    )
    parser.add_argument(
        "--checkpoint-dir",
        type=Path,
        help="Weekday gaussian_mlp dir containing latest.safetensors (torch export)",
    )
    args = parser.parse_args(argv)

    if args.obs_dim <= 0:
        raise SystemExit("--obs-dim must be > 0")
    if path_is_retired(args.output) or (
        args.checkpoint_dir is not None and path_is_retired(args.checkpoint_dir)
    ):
        raise SystemExit("refusing retired unit-lot checkpoint; do not resume fossils")

    if args.checkpoint_dir is not None and args.mean is None:
        export_checkpoint_mu(args.checkpoint_dir, args.output, args.obs_dim)
        print(f"wrote weekday μ ONNX {args.output}")
        return 0

    if args.mean is None:
        raise SystemExit("pass --mean <float> or --checkpoint-dir <gaussian_mlp>")

    if args.checkpoint_dir is not None:
        print(
            f"warn: writing recorded-mean stand-in {args.mean}; "
            "omit --mean to export weekday weights with torch",
            file=sys.stderr,
        )

    write_recorded_mean_onnx(args.output, args.obs_dim, args.mean)
    print(f"wrote recorded-mean stand-in μ={args.mean} → {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
