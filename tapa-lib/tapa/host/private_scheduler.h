// Private implementation header: coroutine worker and thread-pool scheduler.
// Included only from task.cpp inside the #if TAPA_ENABLE_COROUTINE block.
// Not part of the public API.

#pragma once

// --- worker ----------------------------------------------------------------
// A single OS thread that owns a set of coroutines and round-robins them.

class worker {
  // list for stable pointer (used in handle_table)
  std::list<push_type> coroutines;
  unordered_map<push_type*, pull_type*> handle_table;

  std::queue<function<void()>> tasks;
  mutex mtx;
  condition_variable task_cv;
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
            function<void()> f = this->tasks.front();
            this->tasks.pop();

            auto coroutine = new push_type*;
            auto call_back = [this, f, coroutine](pull_type& handle) {
              this->handle_table[*coroutine] = current_handle = &handle;
              delete coroutine;
              f();
            };
            // Use 8 MB per coroutine so tasks with large local arrays do not
            // overflow the small default stack.
            this->coroutines.emplace_back(coroutine_stack(8 * 1024 * 1024),
                                          call_back);
            *coroutine = &this->coroutines.back();
          }
        }

        bool coroutine_executed = false;

        bool debugging = this->signal;
        if (debugging) debug = true;
        for (auto it = this->coroutines.begin();
             it != this->coroutines.end();) {
          if (auto& coroutine = *it) {
            current_handle = this->handle_table[&coroutine];
            coroutine();
            coroutine_executed = true;
          }

          if (*it) {
            ++it;
          } else {
            unique_lock lock(this->mtx);
            it = this->coroutines.erase(it);
          }
        }

        if (debugging) {
          debug = false;
          this->signal = 0;
        }

        if (coroutine_executed) {
          // Running a coroutine is progress for the backoff: start the next
          // idle spell at the cheap end rather than wherever the last one
          // ended. It is NOT stream progress: a blocked coroutine runs (one
          // poll, one yield) every round, so resetting blocked_poll_count
          // here would keep the stall detector from ever firing in this
          // build. Only a successful poll resets that one.
          blocked_poll_streak = 0;
        } else {
          reschedule_this_thread();
        }
      }
    });
  }

  void add_task(const function<void()>& f) {
    {
      unique_lock lock(this->mtx);
      this->tasks.emplace(f);
    }
    this->task_cv.notify_one();
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
  condition_variable wait_cv;
  size_t joined_tasks = 0;
  std::list<worker> workers;
  decltype(workers)::iterator it;

  mutex cleanup_mtx;
  std::list<function<void()>> cleanup_tasks;

 public:
  thread_pool() {
    signal(SIGINT, signal_handler);
    ensure_split_stack_runtime_ready();
    size_t worker_count;
    if (auto concurrency = getenv("TAPA_CONCURRENCY")) {
      worker_count = atoi(concurrency);
    } else {
      worker_count = get_physical_core_count();
    }
    if (worker_count == 0) worker_count = 1;
    for (size_t i = 0; i < worker_count; ++i) workers.emplace_back();
    it = workers.begin();
  }

  void add_task(bool detach, const function<void()>& f) {
    unique_lock lock(this->worker_mtx);
    if (detach) {
      it->add_task(f);
    } else {
      it->add_task([this, f] {
        f();
        unique_lock lock(this->worker_mtx);
        if (--this->joined_tasks == 0) this->wait_cv.notify_all();
      });
      // The worker cannot decrement this until worker_mtx is released.
      // Count only after enqueueing succeeds.
      ++this->joined_tasks;
    }
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
    // A running upper task can enqueue children on any worker, including
    // one that was idle earlier. Its children are counted before it returns.
    unique_lock lock(this->worker_mtx);
    this->wait_cv.wait(lock, [this] { return this->joined_tasks == 0; });
  }

  void send(int signal) {
    for (auto& worker : this->workers) worker.send(signal);
  }

  ~thread_pool() {
    unique_lock lock(this->worker_mtx);
    this->workers.clear();
  }
};
