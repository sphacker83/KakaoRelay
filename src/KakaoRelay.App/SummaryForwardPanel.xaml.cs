using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Windows;
using System.Windows.Controls;
using KakaoRelay.Core;

namespace KakaoRelay.App;

public partial class SummaryForwardPanel : UserControl, INotifyPropertyChanged
{
    public sealed class Choice { public required SummaryRoom Room { get; init; } public bool Selected { get; set; } }
    private AiService? ai;
    private Func<AiSettings>? settings;
    private Func<bool>? save;
    private SummaryForwarder? forwarder;
    private readonly CancellationTokenSource lifetime = new();
    private readonly System.Windows.Threading.DispatcherTimer timer = new() { Interval = TimeSpan.FromSeconds(15) };
    private Task running = Task.CompletedTask;
    private CancellationTokenSource? runCancellation;
    private bool loading, stopping;
    private SummaryForwardRule? selectedRule;
    public event PropertyChangedEventHandler? PropertyChanged;
    public ObservableCollection<SummaryForwardRule> Rules { get; } = [];
    public ObservableCollection<Choice> Sources { get; } = [];
    public ObservableCollection<SummaryRoom> Destinations { get; } = [];
    public SummaryForwardRule? SelectedRule { get => selectedRule; set { selectedRule = value; LoadDraft(); } }
    public SummaryRoom? Destination { get; set; }
    public bool Enabled { get; set; }
    public int IntervalMinutes { get; set; } = 30;
    public bool Busy => !running.IsCompleted || loading;
    public bool CanEdit => ai is not null && !loading && !stopping;
    public string Status { get; private set; } = "원본 방·수신방·주기를 지정하고 적용하세요. 처음 켠 이후의 새 대화부터 전달합니다.";
    public SummaryForwardPanel()
    {
        InitializeComponent(); DataContext = this;
        timer.Tick += (_, _) => StartTick();
        IsVisibleChanged += async (_, _) => { if (IsVisible && CanEdit) await RefreshRooms(); };
    }
    public void Initialize(AiService service, Func<AiSettings> getSettings, Func<bool> saveSettings,
        Func<ApiSendCommand, CancellationToken, Task<TestSendReceipt>> send)
    {
        ai = service; settings = getSettings; save = saveSettings;
        foreach (var rule in settings().SummaryForwards) Rules.Add(rule);
        forwarder = new(ai, send, ConversationCatalog.Scan);
        forwarder.StatusChanged += message => Dispatcher.Invoke(() => { Status = message; Changed(); });
        SelectedRule = Rules.FirstOrDefault();
        timer.Start(); StartTick(); Changed();
    }
    private void Changed() => PropertyChanged?.Invoke(this, new(string.Empty));
    private void LoadDraft()
    {
        Enabled = selectedRule?.Enabled ?? false;
        IntervalMinutes = selectedRule?.IntervalMinutes ?? 30;
        foreach (var source in selectedRule?.Sources ?? [])
            if (!Sources.Any(c => c.Room.Key == source.Key)) Sources.Add(new() { Room = source });
        foreach (var choice in Sources) choice.Selected = selectedRule?.Sources.Any(r => r.Key == choice.Room.Key) == true;
        // Rebind plain draft choices so checkbox state is refreshed when switching rules.
        var choices = Sources.ToList(); Sources.Clear(); foreach (var choice in choices) Sources.Add(choice);
        var dest = selectedRule?.Destination;
        if (dest is not null && !Destinations.Any(d => d.Key == dest.Key)) Destinations.Add(dest);
        Destination = Destinations.FirstOrDefault(d => d.Key == dest?.Key);
        Changed();
    }
    private async Task RefreshRooms()
    {
        if (!CanEdit) return;
        loading = true; Changed();
        try
        {
            var selected = Sources.Where(s => s.Selected).Select(s => s.Room.Key).ToHashSet();
            if (Sources.Count == 0 && selectedRule is not null) selected.UnionWith(selectedRule.Sources.Select(s => s.Key));
            var dest = Destination;
            var rooms = await ai!.Chats.RoomsAsync(lifetime.Token);
            var windows = await Task.Run(ConversationCatalog.Scan, lifetime.Token);
            Sources.Clear(); Destinations.Clear();
            foreach (var room in rooms.Where(r => r.Readable || selected.Contains(AiSettings.RoomKey(r.Profile, r.Id))))
            {
                var source = new SummaryRoom(room.Profile, room.Id, room.Title);
                Sources.Add(new() { Room = source, Selected = selected.Contains(source.Key) });
            }
            foreach (var room in AutoReplyReadiness.OpenRooms(rooms, windows)) Destinations.Add(new(room.Profile, room.Id, room.Title));
            if (dest is not null && !Destinations.Any(d => d.Key == dest.Key)) Destinations.Add(dest);
            Destination = Destinations.FirstOrDefault(d => d.Key == dest?.Key);
        }
        catch (OperationCanceledException) { }
        catch (Exception error) { Status = error.Message; }
        finally { loading = false; Changed(); }
    }
    private async void Refresh_Click(object sender, RoutedEventArgs e) => await RefreshRooms();
    private void New_Click(object sender, RoutedEventArgs e) { SelectedRule = null; Status = "새 규칙 · 방과 주기를 지정하세요."; Changed(); }
    private void Apply_Click(object sender, RoutedEventArgs e)
    {
        if (!CanEdit) return;
        try
        {
            if (!int.TryParse(IntervalInput.Text, out var interval)) throw new ArgumentException("주기는 1~1440분의 숫자로 입력하세요.");
            var next = new SummaryForwardRule { Enabled = Enabled, IntervalMinutes = interval,
                Sources = Sources.Where(s => s.Selected).Select(s => s.Room).ToList(), Destination = Destination };
            next.Validate();
            var old = SelectedRule;
            // Retain progress for interval changes; new routes/re-enabling start at a fresh baseline.
            if (old is not null && (!next.Enabled || old.Enabled) && old.Destination?.Key == next.Destination?.Key
                && old.Sources.Select(s => s.Key).Order().SequenceEqual(next.Sources.Select(s => s.Key).Order())) next.Id = old.Id;
            var previous = settings!().SummaryForwards;
            settings().SummaryForwards = previous.Where(r => r.Id != old?.Id).Append(next).ToList();
            if (save!() != true) { settings().SummaryForwards = previous; throw new InvalidOperationException("저장 실패"); }
            Rules.Clear(); foreach (var rule in settings().SummaryForwards) Rules.Add(rule);
            SelectedRule = next;
            runCancellation?.Cancel();
            Status = next.Enabled ? "사용 · 추가사항만 지정 주기로 전달합니다. 수신방을 열어두세요." : "전달 꺼짐";
            StartTick(); Changed();
        }
        catch (Exception error) { Status = error.Message; Changed(); }
    }
    private void Delete_Click(object sender, RoutedEventArgs e)
    {
        if (!CanEdit || SelectedRule is null) return;
        var old = settings!().SummaryForwards;
        settings().SummaryForwards = old.Where(r => r.Id != SelectedRule.Id).ToList();
        if (save!() != true) { settings().SummaryForwards = old; return; }
        Rules.Remove(SelectedRule); SelectedRule = Rules.FirstOrDefault(); Changed();
        runCancellation?.Cancel();
    }
    private void StartTick()
    {
        if (stopping || Busy || forwarder is null) return;
        running = RunTick(); Changed();
    }
    private async Task RunTick()
    {
        using var cancellation = CancellationTokenSource.CreateLinkedTokenSource(lifetime.Token);
        runCancellation = cancellation;
        try { await forwarder!.TickAsync(cancellation.Token); }
        catch (OperationCanceledException) { }
        catch (Exception error) { Status = error.Message; }
        finally { runCancellation = null; _ = Dispatcher.BeginInvoke(Changed); }
    }
    public void Stop() { stopping = true; timer.Stop(); lifetime.Cancel(); Changed(); }
    public Task DrainAsync() => running;
}
