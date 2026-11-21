// A case of the API's test suite (docs/dev/compatibility.md): `nsh/widget`
// in a run without a packaged widget extension (the compat runner):
// `Widget.available()` is false, host calls reject with
// ERR_FEATURE_UNAVAILABLE_ON_PLATFORM, `defineWidget` preserves its input,
// `Widget.on("action", ...)` returns an unsubscribe function, and
// `<WidgetAction>` / `<WidgetDate>` render without error.
import { Text, View, signal } from "nsh";
import { Widget, WidgetAction, WidgetDate, defineWidget } from "nsh/widget";

function App() {
  const available = signal("pending");
  Widget.available().then(
    (ok) => available.set(String(ok)),
    () => available.set("rejected"),
  );
  const def = defineWidget({
    placeholder: { count: 1 },
    render: ({ data }) => <Text>{String(data?.count ?? 0)}</Text>,
  });
  const defOk = typeof def.render === "function" && def.placeholder?.count === 1;
  const stop = Widget.on("action", () => {});
  const unsubOk = typeof stop === "function";
  if (unsubOk) stop();

  const updateErr = signal("pending");
  const dataVal = signal("pending");
  Widget.update("portfolio", { count: 1 }).then(
    () => updateErr.set("resolved"),
    (err: { code?: string }) => updateErr.set(err?.code ?? "rejected"),
  );
  Widget.getData("portfolio").then(
    (v) => dataVal.set(v === null ? "null" : "value"),
    () => dataVal.set("rejected"),
  );

  return (
    <View style={{ flex: 1, paddingTop: 64 }}>
      <Text>{`Widget available: ${available()}`}</Text>
      <Text>{`Widget define: ${defOk}`}</Text>
      <Text>{`Widget unsub: ${unsubOk}`}</Text>
      <Text>{`Widget update: ${updateErr()}`}</Text>
      <Text>{`Widget getData: ${dataVal()}`}</Text>
      <WidgetAction action="refresh" accessibilityLabel="Refresh widget">
        <Text>Widget action child</Text>
      </WidgetAction>
      <WidgetDate date={1795280000000} mode="relative" />
    </View>
  );
}

export default App;
