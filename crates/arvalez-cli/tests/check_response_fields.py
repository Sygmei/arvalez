from pydantic import ValidationError
from sdk.models import Widget, WidgetResponse, Problem

request = Widget(id=1)
assert request.title == "draft"
assert "nickname" not in request.model_dump(exclude_unset=True)
request.nickname = None
assert request.model_dump(exclude_unset=True)["nickname"] is None

payload = dict(id=1, title="", nickname=None, enabled=False, quantity=0,
               labels=[], metadata={}, blob=None, state=None, child=None,
               partial={}, choice=None, pair=None)
response = WidgetResponse.model_validate(payload)
assert response.title == "" and response.nickname is None
assert response.labels == [] and response.metadata == {}
assert response.model_dump()["nickname"] is None
assert Problem(message=None).message is None
for key in ("title", "nickname", "enabled", "labels"):
    missing = {k: v for k, v in payload.items() if k != key}
    try:
        WidgetResponse.model_validate(missing)
    except ValidationError:
        pass
    else:
        raise AssertionError(f"accepted missing {key}")
try:
    WidgetResponse.model_validate({**payload, "title": None})
except ValidationError:
    pass
else:
    raise AssertionError("accepted null non-nullable field")
