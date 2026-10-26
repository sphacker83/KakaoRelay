using System.Text.Json;
using System.Text.RegularExpressions;

namespace KakaoRelay.Core;

public sealed record GrokTask(string Task, string Reason);
public sealed record GrokCompletion(string JobId, string Request, GrokTask Task, string Result);
public static class GrokDelegation
{
    public const string Instructions = "너는 대화 담당 Codex 코디네이터다. 일상 대화, 간단한 질문과 코딩은 직접 답하라. 현재 요청이 복잡한 분석, 심도 깊은 추론, 어려운 코딩이며 전문가 검토가 필요하면 Grok에 위임할 수 있다. 위임할 때 먼저 사용자에게 짧게 검토를 맡기고 대화를 계속할 수 있다고 알린 뒤, 마지막 줄(감정 표시 앞)에 [[delegate_grok:{\"task\":\"필요한 문맥·조건·코드를 포함한 독립적인 작업 설명\",\"reason\":\"위임 이유\"}]]를 한 번만 붙여라. task와 reason은 각각 JSON 문자열이어야 한다. 관련 없는 대화나 개인정보는 포함하지 마라. Grok은 별도 백그라운드 작업이고 결과가 오면 다시 전달받는다. 아직 완료됐다고 말하지 마라. 과거 요청이나 진행 중인 작업을 중복 위임하지 마라. 이미지 생성과 위임을 동시에 요청하지 마라. 매 턴의 delegationAllowed가 false이면 위임 표시를 절대 출력하지 마라.";

    public static (string Text, GrokTask? Task) Parse(string text)
    {
        var match = Regex.Match(text, "(?:^|\\r?\\n)\\[\\[delegate_grok:(.+)\\]\\]\\s*$");
        if (!match.Success)
        {
            if (text.Contains("[[delegate_grok:", StringComparison.Ordinal)) throw new InvalidDataException("Grok 위임 요청 형식이 올바르지 않습니다.");
            return (text.Trim(), null);
        }
        var task = JsonSerializer.Deserialize<GrokTask>(match.Groups[1].Value, new JsonSerializerOptions(JsonSerializerDefaults.Web));
        var answer = text[..match.Index].Trim();
        if (task is null || string.IsNullOrWhiteSpace(task.Task) || task.Task.Length > 30000 || string.IsNullOrWhiteSpace(task.Reason)
            || task.Reason.Length > 1000 || task.Task.Contains('\0') || answer.Contains("[[delegate_grok:", StringComparison.Ordinal))
            throw new InvalidDataException("Grok 위임 작업과 이유를 확인하세요.");
        return (answer, task);
    }
}
