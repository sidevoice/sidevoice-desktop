import AVFoundation
import Foundation
import SwiftRs
import Tauri
import UIKit
import WebKit

class StartArgs: Decodable {
  let dir: String?
}

/// S0 spike, run N on iOS: the call's native audio held by an `AVAudioSession` in `playAndRecord`, an `AVAudioEngine`
/// input tap and a silent output (an `AVAudioSourceNode` rendering zeros), with nothing done with either but
/// counting. Every 5 s it writes what it counted to `holder.jsonl` in the run directory, with the wall clock (`t`, ms)
/// and the uptime (`up`, ms). In both runs, `watch` writes interruptions, route changes, lock (protected data) and app
/// state to `system.jsonl`. Called only by the app's Rust (`run_mobile_plugin`). Nothing here is product code.
class HolderPlugin: Plugin {
  private let lock = NSLock()
  private var engine: AVAudioEngine?
  private var source: AVAudioSourceNode?
  private var timer: DispatchSourceTimer?
  private var log: FileHandle?
  private var systemLog: FileHandle?
  private var observers: [NSObjectProtocol] = []
  private var captureFrames: Int64 = 0
  private var renderFrames: Int64 = 0
  private var capturePeak: Float = 0
  private var running = false
  private var lastError: String?

  @objc public func logDir(_ invoke: Invoke) throws {
    let dir = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
    invoke.resolve(["path": dir.path])
  }

  /// Starts logging what the phone does around a run (lock, app state, audio session), in both runs, to
  /// `system.jsonl` in the run directory.
  @objc public func watch(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(StartArgs.self)
    guard let dir = args.dir else {
      invoke.reject("no run directory")
      return
    }
    DispatchQueue.main.async {
      self.unwatchNow()
      self.systemLog = self.open(dir: dir, name: "system.jsonl")
      self.writeSystem("watch-start", ["protected_data": UIApplication.shared.isProtectedDataAvailable])
      self.watchSystem()
      invoke.resolve(["watching": true])
    }
  }

  @objc public func unwatch(_ invoke: Invoke) throws {
    DispatchQueue.main.async {
      self.unwatchNow()
      invoke.resolve(["watching": false])
    }
  }

  @objc public func start(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(StartArgs.self)
    guard let dir = args.dir else {
      invoke.reject("no run directory")
      return
    }
    if running {
      write("holder-start-ignored", ["reason": "already running"])
      invoke.resolve(["started": true])
      return
    }
    AVAudioSession.sharedInstance().requestRecordPermission { granted in
      DispatchQueue.main.async {
        guard granted else {
          invoke.reject("microphone permission denied")
          return
        }
        do {
          try self.begin(dir: dir)
          invoke.resolve(["started": true])
        } catch {
          self.lastError = "\(error)"
          self.write("holder-error", ["message": "\(error)"])
          invoke.reject("the holder did not start: \(error)")
        }
      }
    }
  }

  @objc public func stop(_ invoke: Invoke) throws {
    end()
    invoke.resolve(["stopped": true])
  }

  @objc public func status(_ invoke: Invoke) throws {
    lock.lock()
    let answer: [String: Any] = [
      "running": running, "captureFrames": captureFrames, "renderFrames": renderFrames,
      "error": lastError as Any,
    ]
    lock.unlock()
    invoke.resolve(answer)
  }

  private func begin(dir: String) throws {
    log = open(dir: dir, name: "holder.jsonl")

    let session = AVAudioSession.sharedInstance()
    try session.setCategory(.playAndRecord, mode: .default, options: [.defaultToSpeaker, .allowBluetooth])
    try session.setActive(true)

    let engine = AVAudioEngine()
    let input = engine.inputNode
    let inFormat = input.outputFormat(forBus: 0)
    input.installTap(onBus: 0, bufferSize: 4096, format: inFormat) { [weak self] buffer, _ in
      guard let self = self else { return }
      var peak: Float = 0
      if let samples = buffer.floatChannelData?[0] {
        for i in 0..<Int(buffer.frameLength) { peak = max(peak, abs(samples[i])) }
      }
      self.lock.lock()
      self.captureFrames += Int64(buffer.frameLength)
      self.capturePeak = max(self.capturePeak, peak)
      self.lock.unlock()
    }
    let outFormat = engine.outputNode.inputFormat(forBus: 0)
    let source = AVAudioSourceNode { [weak self] _, _, frameCount, audioBufferList in
      let buffers = UnsafeMutableAudioBufferListPointer(audioBufferList)
      for buffer in buffers {
        if let data = buffer.mData { memset(data, 0, Int(buffer.mDataByteSize)) }
      }
      if let self = self {
        self.lock.lock()
        self.renderFrames += Int64(frameCount)
        self.lock.unlock()
      }
      return noErr
    }
    engine.attach(source)
    engine.connect(source, to: engine.mainMixerNode, format: AVAudioFormat(standardFormatWithSampleRate: outFormat.sampleRate, channels: 1))
    try engine.start()
    self.engine = engine
    self.source = source
    running = true
    write("holder-start", [
      "device": UIDevice.current.model, "system": "\(UIDevice.current.systemName) \(UIDevice.current.systemVersion)",
      "input_rate": inFormat.sampleRate, "output_rate": outFormat.sampleRate,
    ])
    tick()
  }

  private func open(dir: String, name: String) -> FileHandle? {
    let url = URL(fileURLWithPath: dir).appendingPathComponent(name)
    if !FileManager.default.fileExists(atPath: url.path) {
      FileManager.default.createFile(atPath: url.path, contents: nil)
    }
    let handle = try? FileHandle(forWritingTo: url)
    handle?.seekToEndOfFile()
    return handle
  }

  private func unwatchNow() {
    for observer in observers { NotificationCenter.default.removeObserver(observer) }
    observers = []
    if systemLog != nil { writeSystem("watch-stop", [:]) }
    lock.lock()
    try? systemLog?.close()
    systemLog = nil
    lock.unlock()
  }

  private func watchSystem() {
    let center = NotificationCenter.default
    let events: [(Notification.Name, String)] = [
      (AVAudioSession.interruptionNotification, "audio-interruption"),
      (AVAudioSession.routeChangeNotification, "audio-route-change"),
      (AVAudioSession.mediaServicesWereResetNotification, "audio-services-reset"),
      (UIApplication.protectedDataWillBecomeUnavailableNotification, "locked"),
      (UIApplication.protectedDataDidBecomeAvailableNotification, "unlocked"),
      (UIApplication.didEnterBackgroundNotification, "app-background"),
      (UIApplication.willEnterForegroundNotification, "app-foreground"),
      (UIApplication.willTerminateNotification, "app-terminate"),
      (.AVAudioEngineConfigurationChange, "engine-configuration-change"),
    ]
    for (name, kind) in events {
      observers.append(center.addObserver(forName: name, object: nil, queue: nil) { [weak self] note in
        var data: [String: Any] = [:]
        if let info = note.userInfo {
          for (key, value) in info { data["\(key)"] = "\(value)" }
        }
        self?.writeSystem(kind, data)
      })
    }
  }

  private func tick() {
    let timer = DispatchSource.makeTimerSource(queue: DispatchQueue.global(qos: .utility))
    var lastCapture: Int64 = 0
    var lastRender: Int64 = 0
    timer.schedule(deadline: .now() + 5, repeating: 5)
    timer.setEventHandler { [weak self] in
      guard let self = self else { return }
      self.lock.lock()
      let capture = self.captureFrames
      let render = self.renderFrames
      let peak = self.capturePeak
      self.capturePeak = 0
      self.lock.unlock()
      let running = self.engine?.isRunning ?? false
      DispatchQueue.main.async {
        let state = UIApplication.shared.applicationState
        let protected = UIApplication.shared.isProtectedDataAvailable
        self.write("holder-tick", [
          "capture_frames": capture, "capture_delta": capture - lastCapture, "capture_peak": peak,
          "render_frames": render, "render_delta": render - lastRender, "engine_running": running,
          "app_state": state == .active ? "active" : state == .background ? "background" : "inactive",
          "protected_data": protected,
        ])
        lastCapture = capture
        lastRender = render
      }
    }
    timer.resume()
    self.timer = timer
  }

  private func end() {
    guard running else { return }
    running = false
    timer?.cancel()
    timer = nil
    engine?.inputNode.removeTap(onBus: 0)
    engine?.stop()
    engine = nil
    source = nil
    try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    write("holder-stop", ["capture_frames": captureFrames, "render_frames": renderFrames])
    lock.lock()
    try? log?.close()
    log = nil
    lock.unlock()
  }

  private func write(_ kind: String, _ data: [String: Any]) {
    append(kind, data, src: "holder", system: false)
  }

  private func writeSystem(_ kind: String, _ data: [String: Any]) {
    append(kind, data, src: "system", system: true)
  }

  private func append(_ kind: String, _ data: [String: Any], src: String, system: Bool) {
    var line = data
    line["t"] = Int64(Date().timeIntervalSince1970 * 1000)
    line["up"] = Int64(ProcessInfo.processInfo.systemUptime * 1000)
    line["src"] = src
    line["k"] = kind
    guard JSONSerialization.isValidJSONObject(line),
      let json = try? JSONSerialization.data(withJSONObject: line)
    else { return }
    lock.lock()
    let handle = system ? systemLog : log
    handle?.write(json)
    handle?.write("\n".data(using: .utf8)!)
    lock.unlock()
  }
}

@_cdecl("init_plugin_s0_holder")
public func initPlugin() -> Plugin {
  return HolderPlugin()
}
