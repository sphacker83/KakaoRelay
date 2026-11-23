using System.Net;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using KakaoRelay.Core;

internal static class AttachmentChecks
{
    private sealed class Handler(byte[] bytes) : HttpMessageHandler
    {
        public int Calls;
        public HttpStatusCode Status = HttpStatusCode.OK;
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken ct)
        { Calls++; return Task.FromResult(new HttpResponseMessage(Status) { Content = new ByteArrayContent(bytes) }); }
    }
    public static async Task RunAsync(string root, Action<bool,string> check)
    {
        var bytes = Encoding.UTF8.GetBytes("회의 결정: 금요일까지 제출");
        var handler = new Handler(bytes); using var client = new HttpClient(handler);
        var reader = new ReceivedAttachments(client, Path.Combine(root,"attachments"));
        LocalMessage Message(string url = "https://talk.kakaocdn.net/test", string? hash = null) => new("1","human","작성자",DateTimeOffset.UtcNow,18,"회의.txt",false)
        { AttachmentMetadata = JsonSerializer.Serialize(new { name="회의.txt",size=bytes.Length,cs=hash??Convert.ToHexString(SHA1.HashData(bytes)),url }) };
        var m = Message(); var result = await reader.ReadAsync(m);
        check(result.Text.Contains("금요일까지 제출") && handler.Calls == 1,"Attachment text is downloaded, verified and extracted");
        await reader.ReadAsync(m); check(handler.Calls == 1,"Verified attachment cache avoids repeat downloads");
        check(!JsonSerializer.Serialize(m).Contains("kakaocdn"),"Attachment URLs never serialize into prompts or API messages");
        check(!ReceivedAttachments.AllowedUrl("http://talk.kakaocdn.net/a") && !ReceivedAttachments.AllowedUrl("https://kakaocdn.net.evil.test/a") && !ReceivedAttachments.AllowedUrl("https://127.0.0.1/a"),"Attachment downloads reject non-HTTPS and unrelated hosts");
        var bad = await reader.ReadAsync(Message(hash:new string('0',40)));
        check(bad.Text.Contains("검증에 실패"),"Mismatched attachment checksum never enters the prompt");
        var untouched = await reader.ReadAsync(m with { Deleted=true });
        check(untouched.Text == m.Text,"Deleted attachments are never downloaded or extracted");
        handler.Status = HttpStatusCode.Found;
        var redirect = await reader.ReadAsync(Message(hash:new string('1',40)));
        check(redirect.Text.Contains("첨부 읽기 실패"),"Redirect response is not accepted as an attachment");
        check(AutoReplySession.Candidates([m],"0","self","").Count==1 && AutoReplySession.Candidates([m],"0","self","@루나").Count==0,"File messages trigger immediate replies without bypassing mention mode");
        handler.Status = HttpStatusCode.OK;
        var photo = m with { Type=2, Text="사진", AttachmentMetadata=JsonSerializer.Serialize(new {s=bytes.Length,cs="",url="https://talk.kakaocdn.net/photo"}) };
        var invalid = await reader.ReadAsync(photo);
        check(invalid.Attachment is null && invalid.Text.Contains("첨부 읽기 실패"), "Invalid image bytes are not attached");
        var imageBytes = Convert.FromBase64String("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aN1EAAAAASUVORK5CYII=");
        using var imageClient = new HttpClient(new Handler(imageBytes));
        var photoReader = new ReceivedAttachments(imageClient, Path.Combine(root,"photos"));
        photo = photo with { AttachmentMetadata=JsonSerializer.Serialize(new {s=imageBytes.Length,cs="",url="https://talk.kakaocdn.net/photo"}) };
        var original = await photoReader.ReadAsync(photo);
        check(original.Attachment is {MediaType:"image/png"} && File.ReadAllBytes(original.Attachment.Path).SequenceEqual(imageBytes), "Original image is preserved byte for byte without OCR");
        check(!JsonSerializer.Serialize(original).Contains(original.Attachment!.Path), "Private attachment paths stay out of serialized messages");
        var inputs = AgentAttachment.CodexInput("test", [original.Attachment!]);
        check(JsonSerializer.Serialize(inputs).Contains("localImage"), "Codex receives native image input");
        var claude = await AgentAttachment.ClaudeInputAsync("test", [original.Attachment!], default);
        using var blocks = JsonDocument.Parse(JsonSerializer.Serialize(claude));
        check(blocks.RootElement[2].GetProperty("source").GetProperty("data").GetString() == Convert.ToBase64String(imageBytes), "Claude receives original image bytes");
        var pdfBytes = Encoding.ASCII.GetBytes("%PDF-1.7\nfixture");
        using var pdfClient = new HttpClient(new Handler(pdfBytes));
        var pdfMessage = m with { AttachmentMetadata = JsonSerializer.Serialize(new {name="scan.pdf",size=pdfBytes.Length,cs=Convert.ToHexString(SHA1.HashData(pdfBytes)),url="https://talk.kakaocdn.net/scan"}) };
        var pdf = await new ReceivedAttachments(pdfClient,Path.Combine(root,"pdf")).ReadAsync(pdfMessage);
        check(pdf.Attachment?.MediaType=="application/pdf" && File.ReadAllBytes(pdf.Attachment.Path).SequenceEqual(pdfBytes), "PDF original is passed without text extraction");
        var pdfInput = PromptJson.Serialize(AgentAttachment.CodexInput("test", [pdf.Attachment!]));
        check(pdfInput.Contains("PDF 원본 경로") && !pdfInput.Contains("localImage"), "Codex receives PDF original path for agent tools, not an image or forced fallback");
        check(CliAiRunner.ResidentArguments(new(){Id="codex"},null,"test",true).Contains("features.shell_tool=true") && CliAiRunner.ResidentArguments(new(){Id="codex"},null,"test").Contains("features.shell_tool=false"), "Local file tools are enabled only for PDF sessions");
        using (var cli = new CliAiRunner())
        {
            var fake = new AiProviderSettings { Id="codex", Executable=Environment.ProcessPath!, Model="fixture" };
            var first = await cli.RunConversationAsync(fake,"read PDF",10,default,Path.Combine(root,"pdf-session"),null,[pdf.Attachment!]);
            var next = await cli.RunConversationAsync(fake,"ordinary followup",10,default,Path.Combine(root,"pdf-session"),first.SessionId,[pdf.Attachment!]);
            check(first.SessionId==next.SessionId,"PDF and ordinary follow-up reuse Luna session with file tools");
        }
        check(JsonSerializer.Serialize(await AgentAttachment.ClaudeInputAsync("test",[pdf.Attachment!],default)).Contains("document"),"Claude receives native PDF document block");
        check(AutoReplySession.Candidates([photo],"0","self","").Count==1 && AutoReplySession.Candidates([photo],"0","self","@루나").Count==0,"Photos respect immediate and mention reply modes");
    }
}
