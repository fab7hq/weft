"""Writes a tiny sense-xs-shaped graph and two adapters for weft-sense's
tests: logit = scale * sum(input_ids * attention_mask) + bias, where scale
and bias are adapter inputs (0 by default). Run with sense's environment
(onnxruntime 1.31): `../../../../../../sense/.venv/bin/python make.py`."""

import numpy as np
import onnx
import onnxruntime as ort
from onnx import TensorProto, helper

ADAPTERS = {"state": (1.0, 0.0), "ask": (-1.0, 0.5)}

ids = helper.make_tensor_value_info("input_ids", TensorProto.INT64, ["B", "L"])
mask = helper.make_tensor_value_info("attention_mask", TensorProto.INT64, ["B", "L"])
params = [helper.make_tensor_value_info(n, TensorProto.FLOAT, [1]) for n in ("scale", "bias")]
logit = helper.make_tensor_value_info("logit", TensorProto.FLOAT, ["B"])
nodes = [
    helper.make_node("Mul", ["input_ids", "attention_mask"], ["kept"]),
    helper.make_node("Cast", ["kept"], ["kept_f"], to=TensorProto.FLOAT),
    helper.make_node("ReduceSum", ["kept_f", "axes"], ["total"], keepdims=0),
    helper.make_node("Mul", ["total", "scale"], ["scaled"]),
    helper.make_node("Add", ["scaled", "bias"], ["logit"]),
]
inits = [
    helper.make_tensor("axes", TensorProto.INT64, [1], [1]),
    helper.make_tensor("scale", TensorProto.FLOAT, [1], [0.0]),
    helper.make_tensor("bias", TensorProto.FLOAT, [1], [0.0]),
]
graph = helper.make_graph(nodes, "sense-xs-fixture", [ids, mask, *params], [logit], inits)
model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 17)], ir_version=8)
onnx.checker.check_model(model)
onnx.save(model, "sense-xs.int8.onnx")

for name, (scale, bias) in ADAPTERS.items():
    # The OrtValues view these arrays: they must live until the file is written.
    arrays = {"scale": np.array([scale], np.float32), "bias": np.array([bias], np.float32)}
    adapter = ort.AdapterFormat()
    adapter.set_adapter_version(1)
    adapter.set_model_version(1)
    adapter.set_parameters({k: ort.OrtValue.ortvalue_from_numpy(v) for k, v in arrays.items()})
    adapter.export_adapter(f"adapters/{name}.onnx_adapter")
