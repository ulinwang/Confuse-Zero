"""Smoke-test the official Python SDK against a local fixture project."""
import base64
import json
import os
import urllib.request
from langfuse import Langfuse, propagate_attributes

sdk = Langfuse(otel_compression="gzip")
try:
    with sdk.start_as_current_observation(name="python-sdk-root") as root:
        with propagate_attributes(session_id="python-sdk-session"):
            with sdk.start_as_current_observation(
                as_type="generation",
                name="python-sdk-model",
                model="fixture-model",
                input={"prompt": "fixture"},
                output="completed",
                usage_details={"input": 5, "output": 2},
            ):
                pass
        trace_id = root.trace_id
    sdk.create_score(trace_id=trace_id, name="fixture-quality", value=1.0)
    sdk.flush()
    result = sdk.api.observations.get_many(trace_id=trace_id, fields="basic,model,usage")
    root = next(row for row in result.data if row.name == "python-sdk-root")
    generation = next(row for row in result.data if row.name == "python-sdk-model")
    assert generation.parent_observation_id == root.id
    assert generation.model == "fixture-model"
    assert generation.usage_details == {"input": 5, "output": 2}, generation
    key = os.environ["LANGFUSE_PUBLIC_KEY"] + ":" + os.environ["LANGFUSE_SECRET_KEY"]
    auth = base64.b64encode(key.encode()).decode()
    url = os.environ["LANGFUSE_BASE_URL"] + "/api/public/scores?traceId=" + trace_id
    req = urllib.request.Request(url, headers={"Authorization": "Basic " + auth})
    with urllib.request.urlopen(req) as response:
        assert json.load(response)["data"][0]["name"] == "fixture-quality"
    print("Langfuse Python 5.2.0 received and read back trace, child usage, and score")
finally:
    sdk.shutdown()
