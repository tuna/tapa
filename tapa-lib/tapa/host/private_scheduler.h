// Private implementation header: coroutine worker and thread-pool scheduler.
// Included only from task.cpp inside the #if TAPA_ENABLE_COROUTINE block.
// Not part of the public API.

#ifndef TAPA_HOST_PRIVATE_SCHEDULER_H_
#define TAPA_HOST_PRIVATE_SCHEDULER_H_

// --- worker ----------------------------------------------------------------
// A single OS thread that owns a set of coroutines and round-robins them.

class worker {
  // keyed by detach flag; list for stable pointer (used in handle_table)
  unordered_map<bool, std::list<push_type>> coroutines;
  unordered_map<push_type*, pull_type*> handle_table;

  std::queue<std::tuple<bool, function<void()>>> tasks;
  mutex mtx;
  condition_variable task_cv;
  condition_variable wait_cv;
  bool done = false;
  std::atomic_int signal{0};
  std::thread thread;

 public:
  worker() {
    this->thread = std::thread([this]() {
      for (;;) {
        {
          unique_lock lock(this->mtx);
          this->task_cv.wait(lock, [this] {
            bool pred =
                this->done || !this->coroutines.empty() || !this->tasks.empty();
            if (!pred) reschedule_this_thread();
            return pred;
          });

          if (this->done && this->tasks.empty()) break;

          while (!this->tasks.empty()) {
            bool detach;
            function<void()> f;
            std::tie(detach, f) = this->tasks.front();
            this->tasks.pop();

            auto& l = this->coroutines[detach];
            auto coroutine = new push_type*;
            auto call_back = [this, f, coroutine](pull_type& handle) {
              this->handle_table[*coroutine] = current_handle = &handle;
              delete coroutine;
              f();
            };
            // Use 8 MB stack per coroutine, matching typical thread stack
            // sizes. Without -fsplit-stacks (e.g., when using Clang),
            // segmented_stack uses a fixed-size allocation and tasks with
            // large local arrays (e.g., 128 KB in graph's ProcElem) would
            // overflow the default ~64 KB stack.
            l.emplace_back(segmented_stack(8 * 1024 * 1024), call_back);
            *coroutine = &l.back();
          }
        }

        bool active = false;
        bool coroutine_executed = false;

        bool debugging = this->signal;
        if (debugging) debug = true;
        for (auto& pair : this->coroutines) {
          bool detach = pair.first;
          auto& coroutines = pair.second;
          for (auto it = coroutines.begin(); it != coroutines.end();) {
            if (auto& coroutine = *it) {
              current_handle = this->handle_table[&coroutine];
              coroutine();
              coroutine_executed = true;
            }

            if (*it) {
              if (!detach) active = true;
              ++it;
            } else {
              unique_lock lock(this->mtx);
              it = coroutines.erase(it);
            }
          }
        }

        if (debugging) {
          debug = false;
          this->signal = 0;
        }

        if (!active) {
          this->wait_cv.notify_all();
        }
        if (!coroutine_executed) {
          reschedule_this_thread();
        }
      }
    });
  }

  void add_task(bool detach, const function<void()>& f) {
    {
      unique_lock lock(this->mtx);
      this->tasks.emplace(detach, f);
    }
    this->task_cv.notify_one();
  }

  void wait() {
    unique_lock lock(this->mtx);
    this->wait_cv.wait(lock, [this] {
      bool pred = this->tasks.empty() && this->coroutines[false].empty();
      if (!pred) reschedule_this_thread();
      return pred;
    });
  }

  void send(int signal) { this->signal = signal; }

  ~worker() {
    {
      unique_lock lock(this->mtx);
      this->done = true;
    }
    this->task_cv.notify_all();
    this->thread.join();
  }
};

// --- thread_pool -----------------------------------------------------------
// Round-robin dispatcher across a fixed set of workers.

void signal_handler(int signal);

class thread_pool {
  mutex worker_mtx;
  std::list<worker> workers;
  decltype(workers)::iterator it;

  mutex cleanup_mtx;
  std::list<function<void()>> cleanup_tasks;

 public:
  thread_pool(size_t worker_count = 0) {
    signal(SIGINT, signal_handler);
    if (worker_count == 0) {
      if (auto concurrency = getenv("TAPA_CONCURRENCY")) {
        worker_count = atoi(concurrency);
      } else {
        worker_count = get_physical_core_count();
      }
    }
    this->add_worker(worker_count);
    it = workers.begin();
  }

  void add_worker(size_t count = 1) {
    unique_lock lock(this->worker_mtx);
    for (size_t i = 0; i < count; ++i) {
      this->workers.emplace_back();
    }
  }

  void add_task(bool detach, const function<void()>& f) {
    unique_lock lock(this->worker_mtx);
    it->add_task(detach, f);
    ++it;
    if (it == this->workers.end()) it = this->workers.begin();
  }

  void add_cleanup_task(const function<void()>& f) {
    unique_lock lock(this->cleanup_mtx);
    this->cleanup_tasks.push_back(f);
  }

  void run_cleanup_tasks() {
    unique_lock lock(this->cleanup_mtx);
    for (auto& task : this->cleanup_tasks) {
      task();
    }
    this->cleanup_tasks.clear();
  }

  void wait() {
    for (auto& w : this->workers) w.wait();
  }

  void send(int signal) {
    for (auto& worker : this->workers) worker.send(signal);
  }

  ~thread_pool() {
    unique_lock lock(this->worker_mtx);
    this->workers.clear();
  }
};

#endif  // TAPA_HOST_PRIVATE_SCHEDULER_H_
