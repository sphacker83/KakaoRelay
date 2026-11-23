using KakaoRelay.Core;
using System.Text.Json;
using System.Windows.Controls;
using System.Windows.Data;
using System.Windows.Threading;

if (args.Contains("models")) { Console.WriteLine("You are not authenticated.\nAvailable models:\n * fixture-grok (default)\n - fixture-other"); return 0; }
if (args.Contains("app-server") || args.Contains("--input-format"))
{
    string? line;
    while ((line = await Console.In.ReadLineAsync()) is not null)
    {
        using var doc = JsonDocument.Parse(line); var m = doc.RootElement;
        if (m.TryGetProperty("method", out var method))
        {
            if (method.GetString() == "initialized") continue;
            if (method.GetString() == "initialize") Console.WriteLine(JsonSerializer.Serialize(new { id = m.GetProperty("id"), result = new { } }));
            else if (method.GetString() == "model/list")
            {
                var second = m.GetProperty("params").GetProperty("cursor").ValueKind == JsonValueKind.String;
                Console.WriteLine(JsonSerializer.Serialize(new { id = m.GetProperty("id"), result = new { data = new[] { new { model = second ? "second-model" : "installed-model", displayName = "Fixture model" } }, nextCursor = second ? null : "next" } }));
            }
            else return 9;
        }
        else if (m.GetProperty("type").GetString() == "control_request") Console.WriteLine(JsonSerializer.Serialize(new { type = "control_response", response = new { subtype = "success", request_id = "models", response = new { models = new[] { new { value = "alias[1m]", displayName = "Fixture alias" } } } } }));
        else return 9;
    }
    return 0;
}
void Check(bool ok, string message) { if (!ok) throw new Exception(message); Console.WriteLine("PASS " + message); }
if (args.Contains("--live"))
{
    foreach (var id in AiSettings.Order)
    {
        var catalog = await ModelCatalog.QueryAsync(new() { Id = id });
        Console.WriteLine(JsonSerializer.Serialize(new { provider = id, models = catalog.Models.Select(m => m.Id), catalog.LoginRequired }));
    }
    return 0;
}
foreach (var id in AiSettings.Order)
{
    var catalog = await ModelCatalog.QueryAsync(new() { Id = id, Executable = Environment.ProcessPath! });
    Check(catalog.Models.Count == (id == "claude" ? 1 : 2), id + " metadata protocol and pagination");
    if (id == "grok") Check(catalog.LoginRequired, "Grok unauthenticated catalog is labeled");
    if (id == "claude") Check(catalog.Models.Single().Id == "alias[1m]", "CLI aliases with context suffix remain valid");
}
try { await ModelCatalog.QueryAsync(new() { Id = "codex", Executable = "C:/missing-fixture.exe" }); throw new Exception("Expected missing executable failure"); } catch (InvalidOperationException) { Console.WriteLine("PASS missing CLI has no static fallback"); }
var completion = new TaskCompletionSource();
var thread = new Thread(() =>
{
    try
    {
        var provider = new AiProviderSettings { Model = "saved-model" };
        var combo = new ComboBox { IsEditable = true, DataContext = provider };
        combo.SetBinding(ItemsControl.ItemsSourceProperty, new Binding("ModelChoices"));
        combo.SetBinding(ComboBox.TextProperty, new Binding("Model") { Mode = BindingMode.TwoWay, UpdateSourceTrigger = UpdateSourceTrigger.PropertyChanged });
        provider.SetModelCatalog(new([new("installed-model", "Fixture")], DateTimeOffset.UtcNow), "loaded");
        Dispatcher.CurrentDispatcher.Invoke(() => { }, DispatcherPriority.ContextIdle);
        Check(provider.Model == "saved-model" && combo.Text == "saved-model", "WPF refresh preserves saved selection");
        provider.SetModelCatalog(null, "failed");
        Dispatcher.CurrentDispatcher.Invoke(() => { }, DispatcherPriority.ContextIdle);
        Check(provider.Model == "saved-model" && !provider.ModelChoices.Contains("installed-model"), "failed refresh clears catalog and preserves selection");
        Check(!JsonSerializer.Serialize(provider).Contains("ModelChoices"), "runtime catalog is not persisted as static settings");
        completion.SetResult();
    }
    catch (Exception e) { completion.SetException(e); }
});
thread.SetApartmentState(ApartmentState.STA);thread.Start();await completion.Task;
return 0;
