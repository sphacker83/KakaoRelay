using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Windows;
using System.Windows.Controls;
using KakaoRelay.Core;

namespace KakaoRelay.App;

public partial class AiPanel : UserControl, INotifyPropertyChanged
{
    private AiService? service;
    private Func<ApiSendCommand, CancellationToken, Task<TestSendReceipt>>? send;
    private CancellationTokenSource? operation;
    private LocalRoom? selectedRoom, answerRoom;
    private string answer = "";
    private LocalRoom? summaryRoom;
    private string? summaryPersonaId;
    public bool CanSaveKnowledgeNote => Idle && summaryRoom is not null && summaryPersonaId is not null && !string.IsNullOrWhiteSpace(Answer);
    private bool settingsLoaded, restoringRooms;
    private bool refreshingOpenRooms;
    private string? openWindowSignature;
    private DateTimeOffset lastRoomDiscovery;
    private readonly System.Windows.Threading.DispatcherTimer roomTicker = new() { Interval = TimeSpan.FromSeconds(2) };
    private readonly System.Windows.Threading.DispatcherTimer saveTimer = new() { Interval = TimeSpan.FromMilliseconds(700) };
    private bool generating;
    public Visibility AnswerWaitingVisibility => generating ? Visibility.Visible : Visibility.Collapsed;
    private ReplyRuntime? runtime;
    internal Task WaitForBotsAsync() => Task.WhenAll(runtime?.DrainAsync() ?? Task.CompletedTask, ForwardPanel.DrainAsync(), ControlPanel.DrainAsync());
    private int activeBots => runtime?.Count ?? 0;
    public ObservableCollection<string> BotStatuses { get; } = [];
    public bool BotsRunning => activeBots > 0;
    public bool CanStartBots => service is not null && !BotsRunning && Idle;
    private readonly System.Diagnostics.Stopwatch elapsed = new();
    private readonly System.Windows.Threading.DispatcherTimer ticker = new() { Interval = TimeSpan.FromSeconds(1) };
    public string BotState => BotsRunning ? $"● 자동 답장 가동 중 · {activeBots}개 방" : "○ 자동 답장 꺼짐";
    public string BotBadge => BotsRunning ? $"● 자동답장 {activeBots}개 방" : "○ 자동답장 꺼짐";
    public int PersonaBindingsRevision { get; private set; }
    public string Activity => Busy ? $"{Status} · {elapsed.Elapsed:mm\\:ss} 경과" : Status;
    public Visibility ProgressVisibility => Busy ? Visibility.Visible : Visibility.Collapsed;
    public event PropertyChangedEventHandler? PropertyChanged;
    public event Action<LocalRoom, string>? ComposeRequested;
    public AiSettings Settings { get; private set; } = new();
    private PersonaProfile? editingProfile;
    public PersonaProfile? EditingProfile { get => editingProfile; set { if (value is null || PersonaImages.Busy) return; editingProfile = value; PersonaImages.SwitchPersona(); Changed(); } }
    public PersonaProfile? RoomProfile { get; set; }
    public string RoomReplyMode { get; set; } = "trigger";
    private readonly List<LocalRoom> botSelectionOrder = [];
    public LocalRoom? BotBindingRoom { get; private set; }
    public bool CanBindBot => Idle && BotBindingRoom is not null;
    private void UpdateBotBindingRoom()
    {
        BotBindingRoom = botSelectionOrder.LastOrDefault();
        RoomProfile = BotBindingRoom is null ? null : Settings.ResolvePersona(BotBindingRoom.Profile, BotBindingRoom.Id);
        RoomReplyMode = BotBindingRoom is null ? Settings.ReplyMode : Settings.ReplyModeForRoom(BotBindingRoom.Profile, BotBindingRoom.Id);
        Changed();
    }
    public bool CanEditPersona => Idle && !PersonaImages.Busy;
    public bool ImagesBusy => PersonaImages.Busy;
    public System.Windows.Media.ImageSource? Avatar { get; private set; }
    public string RoomPersonaHint => SelectedRoom is null ? "연결할 방을 선택하세요." : $"현재 봇: {Settings.ResolvePersona(SelectedRoom.Profile, SelectedRoom.Id).Persona.Name} · 호출어 {Settings.TriggerForRoom(SelectedRoom.Profile, SelectedRoom.Id)}";
    public Dictionary<string, string> ReplyModes { get; } = new() { ["immediate"] = "즉시 답변", ["trigger"] = "호출어 답변", ["context"] = "맥락 판단" };
    public string[] ProviderChoices { get; } = ["auto", "codex", "grok", "claude"];
    public string[] EffortChoices { get; } = ["default", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra", "ultracode"];
    public ObservableCollection<LocalRoom> Rooms { get; } = [];
    public ObservableCollection<LocalRoom> OpenBotRooms { get; } = [];
    public LocalRoom? SelectedRoom { get => selectedRoom; set { selectedRoom = value; if (settingsLoaded && !restoringRooms) { Settings.SelectedChatRoom = value is null ? null : AiSettings.RoomKey(value.Profile, value.Id); QueueSave(); } Transcript = ""; ContextHint = "대화 읽기로 문맥을 확인하세요."; Changed(); } }
    public string Question { get; set; } = "";
    public string Answer { get => answer; set { answer = value; Changed(); } }
    public string Transcript { get; private set; } = "";
    public string ContextHint { get; private set; } = "카카오톡에서 원하는 방을 열고 방 새로고침을 누르세요.";
    public string Status { get; private set; } = "설정 준비 중";
    public string CliInfo { get; private set; } = "";
    public string SelfId { get => Settings.SelfId; set { Settings.SelfId = value; QueueSave(); } }
    public string Trigger { get => Settings.Trigger; set { Settings.Trigger = value; QueueSave(); } }
    public bool Busy => operation is not null;
    public bool Idle => !Busy && service is not null && settingsLoaded;
    public bool CanRead => Idle && SelectedRoom is not null;
    public bool CanCompose => Idle && answerRoom is not null && !string.IsNullOrWhiteSpace(Answer);
    public AiPanel()
    {
        InitializeComponent(); DataContext = this; ticker.Tick += (_, _) => Changed();
        roomTicker.Tick += async (_, _) => await RefreshOpenRoomsAsync();
        Loaded += async (_, _) => { roomTicker.Start(); await RefreshOpenRoomsAsync(); };
        Unloaded += (_, _) => roomTicker.Stop();
        saveTimer.Tick += (_, _) => { saveTimer.Stop(); if (settingsLoaded) Save(); };
        AddHandler(TextBox.TextChangedEvent, new TextChangedEventHandler((_, _) => QueueSave()));
        AddHandler(System.Windows.Controls.Primitives.Selector.SelectionChangedEvent, new SelectionChangedEventHandler((_, _) => QueueSave()));
        AddHandler(System.Windows.Controls.Primitives.ToggleButton.CheckedEvent, new RoutedEventHandler((_, _) => QueueSave()));
        AddHandler(System.Windows.Controls.Primitives.ToggleButton.UncheckedEvent, new RoutedEventHandler((_, _) => QueueSave()));
        BotRooms.SelectionChanged += (_, change) =>
        {
            if (!settingsLoaded || restoringRooms) return;
            foreach (var room in change.RemovedItems.OfType<LocalRoom>()) botSelectionOrder.Remove(room);
            foreach (var room in change.AddedItems.OfType<LocalRoom>()) { botSelectionOrder.Remove(room); botSelectionOrder.Add(room); }
            Settings.SelectedBotRooms = botSelectionOrder.Select(r => AiSettings.RoomKey(r.Profile, r.Id)).ToList();
            UpdateBotBindingRoom();
            QueueSave();
        };
    }
    private void QueueSave() { if (!settingsLoaded || restoringRooms) return; saveTimer.Stop(); saveTimer.Start(); }
    public void Initialize(AiService ai, Func<ApiSendCommand, CancellationToken, Task<TestSendReceipt>> sender)
    {
        service = ai; send = sender;
        try { Settings = ai.Settings.Load(); settingsLoaded = true; Status = "준비"; }
        catch (Exception e) { Status = "설정 읽기 실패: " + e.Message; }
        Settings.EnsurePersonas(); editingProfile = Settings.Personas[0];
        foreach (var profile in Settings.Personas) profile.Persona.PropertyChanged += (_, _) => { PersonaBindingsRevision++; QueueSave(); Changed(); };
        PersonaImages.AvatarChanged += avatar => { Avatar = avatar; Changed(); };
        PersonaImages.PropertyChanged += (_, _) => Changed();
        PersonaImages.Initialize(() => EditingProfile!, Save, () => Settings);
        Detect(); Changed();
        ForwardPanel.Initialize(ai, () => Settings, SaveSettings, sender);
        runtime = new(ai, sender);
        runtime.Changed += () => Dispatcher.Invoke(() => { BotStatuses.Clear(); foreach(var status in runtime.Statuses) BotStatuses.Add(status); if(runtime.LastError.Length>0) Status=runtime.LastError; Changed(); });
        ControlPanel.Initialize(ai, () => Settings, ExecuteCommandAsync,
            () => BotBadge + "\n" + string.Join("\n", runtime.Statuses) + "\n" + string.Join("\n", Rooms.Select(r=>r.Title+" · "+r.Id)), sender,
            room => runtime.StopAsync(room));
    }
    private void Changed() => PropertyChanged?.Invoke(this, new(string.Empty));
    internal bool SaveSettings() { var saved = Save(); Changed(); return saved; }
    private bool Save()
    {
        if (!settingsLoaded) return false;
        try { service!.Settings.Save(Settings); return true; }
        catch (Exception e) { Status = e.Message; Changed(); return false; }
    }
    private void Save_Click(object sender, RoutedEventArgs e) { if (Idle && Save()) { Status = "페르소나·프로바이더·지식베이스 설정을 저장했습니다."; Changed(); } }
    private async void SaveKnowledgeNote_Click(object sender, RoutedEventArgs e)
    {
        if (!CanSaveKnowledgeNote || !Save()) return;
        var room = summaryRoom!;
        var knowledge = Settings.Personas.Single(p => p.Id == summaryPersonaId).Knowledge;
        var body = Answer;
        await Work(async ct =>
        {
            var path = await ObsidianKnowledge.SaveNoteAsync(knowledge, room.Title + " 대화 요약", body, room, ct);
            Status = "요약 노트 저장: " + path;
            summaryRoom = null; summaryPersonaId = null;
        });
    }
    private void BindPersona_Click(object sender, RoutedEventArgs e)
    {
        if (!CanBindBot || RoomProfile is null || BotBindingRoom is not { } room || !BotRooms.SelectedItems.Contains(room)) return;
        Settings.RoomPersonas[AiSettings.RoomKey(room.Profile, room.Id)] = RoomProfile.Id;
        Settings.RoomReplyModes[AiSettings.RoomKey(room.Profile, room.Id)] = RoomReplyMode;
        if (Save()) Status = $"{room.Title} · {RoomProfile.Persona.Name} · {ReplyModes[RoomReplyMode]} 저장됨";
        PersonaBindingsRevision++;
        Changed();
    }
    private CancellationTokenSource? catalogCancellation;
    private async void Detect()
    {
        catalogCancellation?.Cancel();
        using var cancellation = new CancellationTokenSource(); catalogCancellation = cancellation;
        var settings = Settings;
        CliInfo = string.Join("\n", CliAiRunner.Status(settings).Select(s => $"{s.Provider}: {s.Detail}")); Changed();
        await Task.WhenAll(settings.Providers.Select(async provider =>
        {
            var snapshot = new AiProviderSettings { Id = provider.Id, Executable = provider.Executable };
            provider.SetModelCatalog(null, "설치된 CLI에서 모델 조회 중…");
            try
            {
                var result = await Task.Run(() => ModelCatalog.QueryAsync(snapshot, cancellation.Token));
                if (cancellation.IsCancellationRequested || Settings != settings || provider.Executable != snapshot.Executable) return;
                provider.SetModelCatalog(result, $"CLI 제공 {result.Models.Count}개 · {result.CheckedAt.ToLocalTime():HH:mm:ss}" + (result.LoginRequired ? " · CLI 로그인 필요" : " · 실행 권한은 실제 응답 시 확인"));
            }
            catch (Exception e)
            {
                if (!cancellation.IsCancellationRequested && Settings == settings && provider.Executable == snapshot.Executable)
                    provider.SetModelCatalog(null, "목록 조회 실패 · " + e.Message);
            }
        }));
        if (catalogCancellation == cancellation) catalogCancellation = null;
    }
    private void Detect_Click(object sender, RoutedEventArgs e) => Detect();
    private async Task Work(Func<CancellationToken, Task> work)
    {
        if (!Idle) return;
        using var cancellation = new CancellationTokenSource(); operation = cancellation; elapsed.Restart(); ticker.Start(); Changed();
        try { await work(cancellation.Token); }
        catch (OperationCanceledException) { Status = "중지했습니다."; }
        catch (Exception error) { Status = "실패: " + error.Message; }
        finally { operation = null; generating = false; ticker.Stop(); elapsed.Stop(); Status += $" · {elapsed.Elapsed.TotalSeconds:F1}초"; Changed(); }
    }
    private async void Rooms_Click(object sender, RoutedEventArgs e) => await Work(async ct =>
    {
        Status = "카카오톡 로컬 대화방 확인 중"; Changed();
        var list = await service!.Chats.RoomsAsync(ct);
        ApplyRooms(list);
        Status = $"열린 방 {OpenBotRooms.Count}개 · 로컬 방 {list.Count}개";
    });
    private static string WindowSignature(List<ConversationTarget> windows) => string.Join("\n",
        windows.Select(w => $"{w.ProcessId}:{w.Handle}:{w.Ambiguous}:{w.Title}").Order(StringComparer.Ordinal));
    private async Task RefreshOpenRoomsAsync()
    {
        if (!IsLoaded || !IsVisible || !Idle || BotsRunning || refreshingOpenRooms) return;
        refreshingOpenRooms = true;
        try
        {
            var windows = await Task.Run(ConversationCatalog.Scan);
            if (!Idle || BotsRunning || !IsLoaded) return;
            var list = Rooms.ToList();
            // A newly opened room can precede the local DB entry/key. Retry discovery
            // without requiring the user to close and reopen either application.
            var missing = windows.Any(w => !list.Any(r => r.Title == w.Title));
            var discover = (list.Count == 0 || missing) && DateTimeOffset.UtcNow - lastRoomDiscovery >= TimeSpan.FromSeconds(30);
            if (discover)
            {
                lastRoomDiscovery = DateTimeOffset.UtcNow;
                list = await service!.Chats.RoomsAsync();
                windows = await Task.Run(ConversationCatalog.Scan);
            }
            if (!Idle || BotsRunning || !IsLoaded) return;
            if (!discover && openWindowSignature == WindowSignature(windows)) return;
            ApplyRooms(list, windows);
            Changed();
        }
        catch (Exception error) { Status = "열린 방 갱신 실패: " + error.Message; Changed(); }
        finally { refreshingOpenRooms = false; }
    }
    private void ApplyRooms(List<LocalRoom> list, List<ConversationTarget>? windows = null)
    {
        windows ??= ConversationCatalog.Scan();
        openWindowSignature = WindowSignature(windows);
        var previous = SelectedRoom;
        restoringRooms = true;
        try
        {
            Rooms.Clear(); foreach (var r in list) Rooms.Add(r);
            OpenBotRooms.Clear();
            foreach (var room in AutoReplyReadiness.OpenRooms(list, windows)) OpenBotRooms.Add(room);
            SelectedRoom = list.FirstOrDefault(r => r.Id == previous?.Id && r.Profile == previous.Profile)
                ?? list.FirstOrDefault(r => AiSettings.RoomKey(r.Profile, r.Id) == Settings.SelectedChatRoom) ?? list.FirstOrDefault(r => r.Readable);
            botSelectionOrder.Clear();
            foreach (var key in Settings.SelectedBotRooms)
            {
                var room = OpenBotRooms.FirstOrDefault(r => AiSettings.RoomKey(r.Profile, r.Id) == key);
                if (room is null || botSelectionOrder.Contains(room)) continue;
                botSelectionOrder.Add(room); BotRooms.SelectedItems.Add(room);
            }
            UpdateBotBindingRoom();
        }
        finally { restoringRooms = false; }
    }
    private async void Read_Click(object sender, RoutedEventArgs e)
    {
        if (!CanRead || !Save()) return; var room = SelectedRoom!;
        await Work(async ct =>
        {
            Status = "로컬 대화 읽는 중"; Changed();
            var context = await service!.Chats.ReadAsync(room.Profile, room.Id, Settings.ContextMessages, ct);
            ApplyRooms(Rooms.Select(r => r.Profile == room.Profile && r.Id == room.Id ? context.Room : r).ToList());
            Transcript = string.Join("\n\n", context.Messages.Select(m => $"{m.Time.ToOffset(TimeSpan.FromHours(9)):MM-dd HH:mm} · {m.Author} ({m.AuthorId})\n{(m.Deleted ? "[삭제된 메시지]" : m.Text)}"));
            ContextHint = $"{context.Messages.Count}개 메시지 · 한국 시간 · 첨부 미디어 제외";
            Status = "로컬 대화 읽기 완료 · 아직 AI에 전달하지 않았습니다.";
        });
    }
    private async void Generate_Click(object sender, RoutedEventArgs e)
    {
        if (!CanRead || !Save()) return;
        var room = SelectedRoom!; var mode = (string)((Button)sender).Tag;
        await Work(async ct =>
        {
            generating = true; answerRoom = null; summaryRoom = null; summaryPersonaId = null; Answer = ""; Status = "대화 문맥으로 AI 응답 생성 중"; Changed();
            var progress = new Progress<string>(message => { if (operation?.Token == ct) { Status = message; Changed(); } });
            var result = await service!.GenerateAsync(new(room.Profile, room.Id, mode, Question), ct, progress);
            ApplyRooms(Rooms.Select(r => r.Profile == room.Profile && r.Id == room.Id ? result.Room : r).ToList());
            Answer = result.Text; answerRoom = mode == "reply" ? result.Room : null;
            summaryRoom = mode == "analyze" ? result.Room : null;
            summaryPersonaId = mode == "analyze" ? result.PersonaId : null;
            Status = $"{result.Provider} · {result.Model} · effort {result.Effort} · {result.MessageCount}개 문맥 · " + string.Join(" → ", result.Attempts.Select(a => $"{a.Provider} {a.Status}"));
            if (result.ImagePath is not null) Status += " · 생성 이미지 저장: " + result.ImagePath;
            if (result.ImageError is not null) Status += " · 이미지 생성 실패: " + result.ImageError;
        });
    }
    private async void Export_Click(object sender, RoutedEventArgs e)
    {
        if (!CanRead || !Save()) return; var room = SelectedRoom!;
        var dialog = new Microsoft.Win32.SaveFileDialog { Filter = "KakaoRelay 대화 JSON|*.json", FileName = "kakao-conversation.json" };
        if (dialog.ShowDialog() != true) return;
        await Work(async ct =>
        {
            var context = await service!.Chats.ReadAsync(room.Profile, room.Id, Settings.ContextMessages, ct);
            await System.IO.File.WriteAllTextAsync(dialog.FileName, System.Text.Json.JsonSerializer.Serialize(context, new System.Text.Json.JsonSerializerOptions(System.Text.Json.JsonSerializerDefaults.Web) { WriteIndented = true, Encoder = PromptJson.Encoder }), ct);
            Status = "선택한 문맥을 JSON으로 저장했습니다. macOS·Linux 버전에서 가져올 수 있습니다.";
        });
    }
    private void Compose_Click(object sender, RoutedEventArgs e) { if (CanCompose) ComposeRequested?.Invoke(answerRoom!, Answer); }
    private async void StartBot_Click(object sender, RoutedEventArgs e)
    {
        if (!CanStartBots || !Save()) return;
        await Work(async ct => {
            var selected=BotRooms.SelectedItems.Cast<LocalRoom>().Where(r=>!ControlPanel.Owns(r)).ToList();
            if(selected.Count==0)throw new InvalidOperationException("자동답장할 방을 선택하세요. 제어방은 제외됩니다.");
            await runtime!.StartAsync(selected,ct); Status="자동답장 시작";
        });
    }
    public event Action? RemoteSettingsChanged;
    internal object RuntimeSnapshot() => new { autoReplyRunning = BotsRunning, activeRooms = activeBots,
        rooms = runtime?.Statuses ?? [], lastError = runtime?.LastError ?? "", commandRoom = ControlPanel.RuntimeStatus };
    internal async Task ResumeSelectedRepliesAsync()
    {
        try
        {
            var rooms = await service!.Chats.RoomsAsync(); ApplyRooms(rooms);
            var selected=rooms.Where(r=>Settings.SelectedBotRooms.Contains(AiSettings.RoomKey(r.Profile,r.Id))&&!ControlPanel.Owns(r)).ToList();
            await runtime!.StartAsync(selected); Status="저장된 자동답장 방 복원 완료"; Changed();
        }
        catch(Exception error){Status="자동답장 복원 실패: "+error.Message;Changed();}
    }
    private async Task<string> ExecuteCommandAsync(CommandPlan plan,CancellationToken ct)
    {
        if(Busy||PersonaImages.Busy)throw new InvalidOperationException("앱에서 다른 설정 작업 중입니다. 잠시 후 다시 요청하세요.");
        if(plan.Revision!=CommandPolicy.Revision(Settings))throw new InvalidOperationException("설정이 바뀌었습니다. 다시 요청하세요.");
        saveTimer.Stop();
        if(plan.Command.Action=="reply")
        {
            if(plan.Command.Value=="켜기")await runtime!.StartAsync([plan.Room!],ct);
            else await runtime!.StopAsync(plan.Room);
        }
        // Do not claim success before the runtime transition or durable save has succeeded.
        try { service!.Settings.Save(plan.Next); }
        catch (Exception error) { throw new InvalidOperationException((plan.Command.Action=="reply"?"자동답장 실행 상태는 변경됐지만 설정 저장 실패: ":"설정 저장 실패: ")+error.Message); }
        var forwardChanged=System.Text.Json.JsonSerializer.Serialize(Settings.SummaryForwards)!=System.Text.Json.JsonSerializer.Serialize(plan.Next.SummaryForwards);
        Settings=plan.Next;
        editingProfile=Settings.Personas.FirstOrDefault(p=>p.Id==editingProfile?.Id)??Settings.Personas[0];
        foreach(var profile in Settings.Personas)profile.Persona.PropertyChanged+=(_,_)=>{PersonaBindingsRevision++;QueueSave();Changed();};
        PersonaBindingsRevision++; ApplyRooms(Rooms.ToList()); if(forwardChanged)ForwardPanel.ReloadSettings(); RemoteSettingsChanged?.Invoke();Changed();
        if(plan.Command.Action=="knowledge-run")
        {
            if(service.AutomaticKnowledge.Busy)return plan.Description+" · 이미 진행 중입니다.";
            await service.AutomaticKnowledge.TickAsync(Settings,true,ct);
            return string.Join("\n",service.AutomaticKnowledge.SavedStatuses(Settings));
        }
        return "적용: "+plan.Description;
    }
    public void Stop() { catalogCancellation?.Cancel(); roomTicker.Stop(); ForwardPanel.Stop(); saveTimer.Stop(); if (settingsLoaded) Save(); operation?.Cancel(); _ = runtime?.StopAsync(); ControlPanel.Stop(); PersonaImages.Stop(); }
    private async void StopBots_Click(object sender, RoutedEventArgs e) { if(runtime is not null) await runtime.StopAsync(); }
    private void Cancel_Click(object sender, RoutedEventArgs e) => operation?.Cancel();
}
