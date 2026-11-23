using System.Diagnostics;
using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;
namespace KakaoRelay.Core;

public sealed record CliModel(string Id, string Label, bool IsDefault = false);
public sealed record CliModelCatalog(IReadOnlyList<CliModel> Models, DateTimeOffset CheckedAt, bool LoginRequired = false);

public static class ModelCatalog
{
    public static bool ValidId(string? id) => id is not null && Regex.IsMatch(id, @"\A[a-zA-Z0-9._:/\[\]-]{1,150}\z");
    public static IReadOnlyList<string> Choices(IEnumerable<CliModel> models, string current = "") =>
        new[] { "" }.Concat(models.Select(m => m.Id)).Concat(string.IsNullOrEmpty(current) ? [] : new[] { current }).Distinct(StringComparer.Ordinal).ToArray();
    public static async Task<CliModelCatalog> QueryAsync(AiProviderSettings provider, CancellationToken cancellation = default)
    {
        if (!AiSettings.Order.Contains(provider.Id)) throw new ArgumentException("지원하지 않는 AI 서비스입니다.");
        var exe = CliAiRunner.Resolve(provider) ?? throw new InvalidOperationException("설치된 CLI 실행 파일을 찾지 못했습니다.");
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(cancellation);
        timeout.CancelAfter(TimeSpan.FromSeconds(30)); var ct = timeout.Token;
        var folder = ProviderStorage.JobFolder(provider.Id, "model-catalog");
        try
        {
            if (provider.Id == "grok") return ParseGrok(await GrokAsync(exe, folder, ct).ConfigureAwait(false));
            var args = provider.Id == "codex" ? new[] { "app-server", "--stdio" } : new[] {
                "--print", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--tools", "",
                "--strict-mcp-config", "--mcp-config", "{\"mcpServers\":{}}", "--no-session-persistence", "--disable-slash-commands", "--setting-sources", "" };
            using var pipe = new CliJsonPipe(exe, folder, args, provider.Id);
            var models = new List<CliModel>();
            if (provider.Id == "codex")
            {
                await pipe.CallAsync("initialize", new { clientInfo = new { name = "kakaorelay-models", version = "1.0.0" } }, ct).ConfigureAwait(false);
                await pipe.SendAsync(new { method = "initialized", @params = new { } }, ct).ConfigureAwait(false);
                string? cursor = null; var cursors = new HashSet<string>();
                for (var page = 0; ; page++)
                {
                    if (page >= 20 || cursor is not null && !cursors.Add(cursor)) throw new IOException("CLI 모델 목록 페이지 오류");
                    var result = await pipe.CallAsync("model/list", new { limit = 100, includeHidden = false, cursor }, ct).ConfigureAwait(false);
                    foreach (var m in result.GetProperty("data").EnumerateArray())
                    {
                        if (m.TryGetProperty("hidden", out var hidden) && hidden.ValueKind == JsonValueKind.True) continue;
                        var id = Text(m, "model") ?? Text(m, "id");
                        if (ValidId(id)) models.Add(new(id!, Text(m, "displayName") ?? id!, m.TryGetProperty("isDefault", out var d) && d.ValueKind == JsonValueKind.True));
                    }
                    cursor = Text(result, "nextCursor"); if (string.IsNullOrEmpty(cursor)) break;
                }
            }
            else
            {
                await pipe.SendAsync(new { type = "control_request", request_id = "models", request = new { subtype = "initialize" } }, ct).ConfigureAwait(false);
                while (true)
                {
                    var result = await pipe.ReadAsync(ct).ConfigureAwait(false);
                    if (Text(result, "type") != "control_response" || !result.TryGetProperty("response", out var response) || Text(response, "request_id") != "models") continue;
                    if (Text(response, "subtype") == "error") throw new IOException("Claude CLI 모델 조회를 확인하지 못했습니다.");
                    foreach (var m in response.GetProperty("response").GetProperty("models").EnumerateArray())
                    { var id = Text(m, "value"); if (ValidId(id)) models.Add(new(id!, Text(m, "displayName") ?? id!, id == "default")); }
                    break;
                }
            }
            return Result(models);
        }
        catch (OperationCanceledException) when (!cancellation.IsCancellationRequested) { throw new TimeoutException("CLI 모델 조회 시간이 초과되었습니다. 로그인 및 설치 상태를 확인하세요."); }
        finally { try { Directory.Delete(folder, true); } catch (IOException) { } catch (UnauthorizedAccessException) { } }
    }
    private static string? Text(JsonElement m, string name) => m.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() : null;
    private static CliModelCatalog Result(IEnumerable<CliModel> models, bool loginRequired = false)
    {
        var unique = models.DistinctBy(m => m.Id).ToArray();
        if (unique.Length == 0) throw new IOException("CLI가 제공한 모델 목록이 비어 있습니다.");
        return new(unique, DateTimeOffset.UtcNow, loginRequired);
    }
    public static CliModelCatalog ParseGrok(string output)
    {
        var text = Regex.Replace(output, "\u001b\\[[0-9;]*m", "");
        var marker = text.IndexOf("Available models:", StringComparison.OrdinalIgnoreCase);
        if (marker < 0) throw new IOException("Grok CLI 모델 목록 응답을 확인하지 못했습니다.");
        var models = Regex.Matches(text[(marker + "Available models:".Length)..], @"(?m)^\s*[*-]\s+([a-zA-Z0-9._:/\[\]-]+)(?:[ \t]+\((default)\))?[ \t]*\r?$")
            .Select(m => new CliModel(m.Groups[1].Value, m.Groups[1].Value, m.Groups[2].Success)).Where(m => ValidId(m.Id));
        return Result(models, text.Contains("not authenticated", StringComparison.OrdinalIgnoreCase));
    }
    private static async Task<string> GrokAsync(string exe, string folder, CancellationToken ct)
    {
        var info = new ProcessStartInfo(exe) { WorkingDirectory = folder, UseShellExecute = false, CreateNoWindow = true,
            RedirectStandardInput = true, RedirectStandardOutput = true, RedirectStandardError = true, StandardOutputEncoding = Encoding.UTF8 };
        ProviderStorage.Configure(info, "grok"); info.ArgumentList.Add("models");
        using var process = Process.Start(info) ?? throw new IOException("Grok CLI 시작 실패");
        process.StandardInput.Close(); var errors = process.StandardError.BaseStream.CopyToAsync(Stream.Null, ct);
        try
        {
            var output = new StringBuilder(); var buffer = new char[4096]; int size;
            while ((size = await process.StandardOutput.ReadAsync(buffer, ct).ConfigureAwait(false)) > 0)
            { output.Append(buffer, 0, size); if (output.Length > 2_000_000) throw new IOException("모델 목록 응답 크기 초과"); }
            await process.WaitForExitAsync(ct).ConfigureAwait(false);
            if (process.ExitCode != 0) throw new IOException("Grok CLI 모델 조회 실패 · 로그인 및 설치 상태를 확인하세요.");
            return output.ToString();
        }
        finally { try { if (!process.HasExited) process.Kill(true); } catch (InvalidOperationException) { } try { await errors.ConfigureAwait(false); } catch (OperationCanceledException) { } catch (IOException) { } }
    }
}
