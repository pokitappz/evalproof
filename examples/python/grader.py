"""A weak invoice grader and its corrected version. No provider calls."""


def weak(output, context):
    return {"pass": isinstance(output, dict) and "invoice_id" in output, "reason": "Only checks that invoice_id exists"}


def strict(output, context):
    return {"pass": output == {"invoice_id": "INV-42", "total": 125}, "reason": "Checks the expected identifier and amount"}
